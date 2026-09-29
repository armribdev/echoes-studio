//! Hardware MIDI input device registry (ADR-022).
//!
//! Enumerates MIDI input ports and opens/closes `midir` connections for the
//! devices the user enables in Settings. `midir` owns and drives its own OS
//! thread per open connection — the connection's callback is the **MIDI
//! capture thread** documented in `docs/active/threading/threading-model.md`.
//! This registry is only ever touched by the command thread; the capture
//! thread itself only reads the armed-track id and pushes into the lock-free
//! `MidiLiveInputQueue` via `LiveParams::push_live_input` (and, while
//! recording, hands the note to the `MidiRecorder` and announces it to the UI) — it never touches this
//! registry or the audio thread.
//!
//! Optionally sends "Local Control Off" to each enabled device's output port
//! of the same name, so a keyboard with its own speakers (digital piano) stops
//! sounding while Echoes plays it. Restored when the device is disabled and on
//! app exit (ADR-022 amendment 2026-09-26).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio_graph::live_params::LiveParams;
use crate::audio_graph::midi::MidiLiveInputMessage;
use crate::midi_recording::{MidiRecorder, MIDI_RECORDING_NOTE_EVENT};
use crate::project::AppState;

const CLIENT_NAME: &str = "Echoes Studio";

/// Emitted whenever a hardware MIDI input device connects or disconnects
/// (ADR-022 amendment: live hotplug detection). Payload: the full
/// `Vec<MidiInputDeviceInfo>`, same shape as `list_midi_input_devices`.
pub const MIDI_DEVICES_EVENT: &str = "midi-devices-changed";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiInputDeviceInfo {
    /// Device name, used both as the display name and as the stable
    /// identifier — midir port indices are not stable across replugs.
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

/// `midir::MidiInputConnection` is not `Send` on every backend (its internal
/// handle mirrors CPAL's `Stream`). It is only ever stored behind
/// `Mutex<MidiInputRegistry>` on the command thread; the connection's own
/// capture thread runs internally, never through this handle. Same pattern
/// and justification as `audio_engine::engine::SendStream`.
struct SendMidiConnection(MidiInputConnection<()>);
unsafe impl Send for SendMidiConnection {}

/// Owns the open hardware MIDI input connections, keyed by device name.
/// Enabling a device opens a `midir` connection whose callback runs on
/// midir's own capture thread for as long as the connection stays open.
pub struct MidiInputRegistry {
    connections: HashMap<String, SendMidiConnection>,
    local_control_off: bool,
    /// Devices switched to Local Control Off by us, so exactly those are
    /// switched back on — never a device the user silenced themselves.
    silenced: HashSet<String>,
}

impl MidiInputRegistry {
    pub fn new() -> Self {
        Self {
            connections: HashMap::new(),
            local_control_off: false,
            silenced: HashSet::new(),
        }
    }

    /// Apply the "silence the keyboard's own sound" preference to every open
    /// device, and to devices enabled later.
    pub fn set_local_control_off(&mut self, off: bool) {
        self.local_control_off = off;
        let open_devices: Vec<String> = self.connections.keys().cloned().collect();
        for device_name in open_devices {
            if off {
                self.silence(&device_name);
            } else {
                self.restore(&device_name);
            }
        }
    }

    /// Give every silenced keyboard its own sound back (app exit).
    pub fn restore_all(&mut self) {
        let silenced: Vec<String> = self.silenced.iter().cloned().collect();
        for device_name in silenced {
            self.restore(&device_name);
        }
    }

    /// Best effort: a device without an output port of the same name simply
    /// keeps its own sound.
    fn silence(&mut self, device_name: &str) {
        if self.silenced.contains(device_name) {
            return;
        }
        match send_local_control(device_name, false) {
            Ok(()) => {
                self.silenced.insert(device_name.to_string());
            }
            Err(e) => eprintln!("[midi] Local Control Off impossible pour {device_name}: {e}"),
        }
    }

    fn restore(&mut self, device_name: &str) {
        if self.silenced.remove(device_name) {
            if let Err(e) = send_local_control(device_name, true) {
                eprintln!("[midi] Local Control On impossible pour {device_name}: {e}");
            }
        }
    }

    /// Drop a stale connection for a device that just disappeared. Reaching
    /// its output to restore Local Control would be pointless (the hardware
    /// is gone), so this only clears our bookkeeping — a later replug is then
    /// seen as "not enabled" and reconnects properly instead of being masked
    /// by a dead `contains_key`.
    fn mark_disconnected(&mut self, device_name: &str) {
        self.connections.remove(device_name);
        self.silenced.remove(device_name);
    }

    /// List currently visible hardware MIDI input ports, flagged with whether
    /// each one currently has an open connection.
    pub fn list_devices(&self) -> Vec<MidiInputDeviceInfo> {
        let Ok(probe) = MidiInput::new(CLIENT_NAME) else {
            return Vec::new();
        };
        probe
            .ports()
            .iter()
            .filter_map(|port| probe.port_name(port).ok())
            .map(|name| MidiInputDeviceInfo {
                enabled: self.connections.contains_key(&name),
                id: name.clone(),
                name,
            })
            .collect()
    }

    /// Open or close the connection for `device_name`. `live_params` and
    /// `armed_track_id` are cloned into the connection callback; from that
    /// point on they are read/written from midir's capture thread as well as
    /// the command thread.
    pub fn set_enabled(
        &mut self,
        device_name: &str,
        enabled: bool,
        live_params: Arc<LiveParams>,
        armed_track_id: Arc<Mutex<Option<String>>>,
        recorder: Arc<MidiRecorder>,
        app: AppHandle,
    ) -> Result<(), String> {
        if !enabled {
            if let Some(SendMidiConnection(connection)) = self.connections.remove(device_name) {
                connection.close();
            }
            self.restore(device_name);
            return Ok(());
        }
        if self.connections.contains_key(device_name) {
            return Ok(());
        }

        let mut midi_in = MidiInput::new(CLIENT_NAME).map_err(|e| e.to_string())?;
        midi_in.ignore(Ignore::ActiveSense);

        let port = midi_in
            .ports()
            .into_iter()
            .find(|port| {
                midi_in
                    .port_name(port)
                    .map(|name| name == device_name)
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("Périphérique MIDI introuvable : {device_name}"))?;

        let connection = midi_in
            .connect(
                &port,
                "echoes-studio-input",
                move |_timestamp, message, _| {
                    // First, before any lock: the stamp must be the arrival time.
                    let received_at = live_params.live_input_clock.now_nanos();
                    let Some(parsed) = parse_note_message(message, received_at) else {
                        return;
                    };
                    let track_id = armed_track_id.lock().ok().and_then(|guard| guard.clone());
                    if let Some(track_id) = track_id {
                        if let Some(note) = recorder.capture(&track_id, &parsed) {
                            let _ = app.emit(MIDI_RECORDING_NOTE_EVENT, note);
                        }
                        live_params.push_live_input(&track_id, parsed);
                    }
                },
                (),
            )
            .map_err(|e| e.to_string())?;

        self.connections
            .insert(device_name.to_string(), SendMidiConnection(connection));
        if self.local_control_off {
            self.silence(device_name);
        }
        Ok(())
    }
}

/// Poll interval for hotplug detection. `midir`/WinMM give no OS-level
/// connect/disconnect callback, so this is a plain re-enumeration loop —
/// cheap (no I/O beyond an OS device query) and only emits when the visible
/// set actually changed.
const HOTPLUG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1000);

/// Spawn the background thread that watches for MIDI devices being plugged
/// or unplugged and pushes `MIDI_DEVICES_EVENT` to the frontend when the
/// visible set changes. Runs for the app's lifetime; never touches the audio
/// thread — this is a command-thread-adjacent poll, not RT code.
pub fn spawn_hotplug_watcher(app: AppHandle) {
    std::thread::Builder::new()
        .name("midi-hotplug-watcher".into())
        .spawn(move || {
            let mut previous: Vec<MidiInputDeviceInfo> = Vec::new();
            loop {
                let state = app.state::<AppState>();
                if let Ok(mut registry) = state.midi_input.lock() {
                    let current = registry.list_devices();
                    let current_names: HashSet<&str> =
                        current.iter().map(|d| d.name.as_str()).collect();
                    for stale in previous
                        .iter()
                        .filter(|d| !current_names.contains(d.name.as_str()))
                    {
                        registry.mark_disconnected(&stale.name);
                    }
                    drop(registry);
                    if current != previous {
                        let _ = app.emit(MIDI_DEVICES_EVENT, &current);
                        previous = current;
                    }
                }
                std::thread::sleep(HOTPLUG_POLL_INTERVAL);
            }
        })
        .expect("failed to spawn midi-hotplug-watcher thread");
}

const CONTROL_CHANGE: u8 = 0xB0;
const LOCAL_CONTROL: u8 = 122;

/// Send Local Control On/Off on all 16 channels to the output port named like
/// the input device. The output connection is opened only for this send.
fn send_local_control(device_name: &str, on: bool) -> Result<(), String> {
    let midi_out = MidiOutput::new(CLIENT_NAME).map_err(|e| e.to_string())?;
    let port = midi_out
        .ports()
        .into_iter()
        .find(|port| {
            midi_out
                .port_name(port)
                .map(|name| name == device_name)
                .unwrap_or(false)
        })
        .ok_or_else(|| format!("pas de sortie MIDI « {device_name} »"))?;
    let mut connection = midi_out
        .connect(&port, "echoes-studio-local-control")
        .map_err(|e| e.to_string())?;
    for message in local_control_messages(on) {
        connection.send(&message).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn local_control_messages(on: bool) -> impl Iterator<Item = [u8; 3]> {
    let value = if on { 127 } else { 0 };
    (0..16u8).map(move |channel| [CONTROL_CHANGE | channel, LOCAL_CONTROL, value])
}

/// Parse a raw MIDI message into a note event. Only Note On / Note Off are
/// handled — CC, pitch bend, program change, etc. are out of scope for this
/// iteration (ADR-022 follow-ups). A Note On with velocity 0 is a Note Off
/// per the MIDI spec.
fn parse_note_message(message: &[u8], received_at: u64) -> Option<MidiLiveInputMessage> {
    let &[status, pitch, velocity] = message else {
        return None;
    };
    match status & 0xF0 {
        0x90 if velocity > 0 => Some(MidiLiveInputMessage {
            note_on: true,
            pitch,
            velocity,
            received_at,
        }),
        0x90 | 0x80 => Some(MidiLiveInputMessage {
            note_on: false,
            pitch,
            velocity: 0,
            received_at,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_on_with_velocity_parses_as_note_on() {
        let message = parse_note_message(&[0x90, 60, 100], 0).unwrap();
        assert!(message.note_on);
        assert_eq!(message.pitch, 60);
        assert_eq!(message.velocity, 100);
    }

    #[test]
    fn note_on_with_zero_velocity_parses_as_note_off() {
        let message = parse_note_message(&[0x90, 60, 0], 0).unwrap();
        assert!(!message.note_on);
    }

    #[test]
    fn note_off_status_parses_as_note_off() {
        let message = parse_note_message(&[0x80, 60, 64], 0).unwrap();
        assert!(!message.note_on);
    }

    #[test]
    fn other_status_bytes_are_ignored() {
        // Control change (0xB0) — out of scope for this iteration.
        assert!(parse_note_message(&[0xB0, 7, 127], 0).is_none());
    }

    #[test]
    fn wrong_length_messages_are_ignored() {
        assert!(parse_note_message(&[0x90, 60], 0).is_none());
    }

    #[test]
    fn local_control_off_is_cc_122_value_0_on_every_channel() {
        let messages: Vec<[u8; 3]> = local_control_messages(false).collect();
        assert_eq!(messages.len(), 16);
        assert_eq!(messages[0], [0xB0, 122, 0]);
        assert_eq!(messages[15], [0xBF, 122, 0]);
    }

    #[test]
    fn local_control_on_is_cc_122_value_127() {
        assert!(local_control_messages(true).all(|m| m[1] == 122 && m[2] == 127));
    }
}
