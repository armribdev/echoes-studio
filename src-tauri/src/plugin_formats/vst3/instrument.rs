//! VST3 instrument adapter — drives a VST3 plugin as an `InstrumentDevice`.
//!
//! ## Role (ADR-013 amendment 2026-10-21)
//!
//! `InstrumentNode` turns clips, the keyboard and the arpeggiator into a
//! per-block `MidiEventList`.  `VstInstrument` hands those notes to a VST3
//! plugin through `ProcessData.inputEvents` and renders its audio output, the
//! same way `ClapInstrument` does for CLAP.
//!
//! ## Real-time contract
//!
//! Mirrors `VstWrapperNode`: every `Vec` / `Box` is allocated in `prepare`;
//! `process_midi` only refills the pre-allocated event and parameter lists and
//! calls `IAudioProcessor::process` — no heap ops, no locks.
//!
//! ## Notes are never left hanging
//!
//! The adapter counts the notes the plugin received.  A NoteOff that could not
//! be delivered (FFI gate busy, event list full) or that follows a `reset` is
//! sent at offset 0 of the next block.

use std::ffi::c_void;
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::{MidiEventKind, MidiEventList};
use crate::audio_graph::node::{PrepareContext, TransportInfo};
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::audio_graph::plugin_params::{PluginParamControl, PluginParamControls};
use crate::plugin_formats::crash_guard;
use crate::plugin_system::instance::{PluginInstance, FFI_GATE_HARVEST_BUDGET};
use crate::plugin_system::lifecycle::LifecycleState;

use super::event_list::HostEventList;
use super::ffi::{AudioBusBuffers, Event, HostParameterChanges, ProcessContext, ProcessData};
use super::midi_mapping::{push_mapped_controls, read_midi_mapping, MidiControllerMap};
use super::param_delivery::{fill_param_changes, track_value_modulation};
use super::transport::vst3_process_context;
use super::wrapper::{vst3_process_trampoline, Vst3ProcessCtx};

/// The plugin matches a NoteOff to its NoteOn by pitch, as the CLAP path does.
const NO_NOTE_ID: i32 = -1;
const MIDI_POLY_PRESSURE: u8 = 0xA0;
const PITCH_COUNT: usize = 128;

/// What the plugin has been told about notes, so none is left sounding.
pub struct SoundingNotes {
    started: [u8; PITCH_COUNT],
    release_pending: [bool; PITCH_COUNT],
}

impl SoundingNotes {
    pub fn new() -> Self {
        SoundingNotes {
            started: [0; PITCH_COUNT],
            release_pending: [false; PITCH_COUNT],
        }
    }

    fn note_started(&mut self, pitch: u8) {
        if let Some(count) = self.started.get_mut(pitch as usize) {
            *count = count.saturating_add(1);
        }
    }

    fn note_ended(&mut self, pitch: u8) {
        if let Some(count) = self.started.get_mut(pitch as usize) {
            *count = count.saturating_sub(1);
        }
    }

    fn keep_release(&mut self, pitch: u8) {
        if let Some(pending) = self.release_pending.get_mut(pitch as usize) {
            *pending = true;
        }
    }

    /// Every note the plugin has not been told to stop is released by the next block.
    pub fn release_all(&mut self) {
        for (pending, &count) in self.release_pending.iter_mut().zip(&self.started) {
            *pending |= count > 0;
        }
    }

    /// A block the plugin did not process: the NoteOffs it would have received
    /// are kept for the next one.
    pub fn keep_releases_of(&mut self, events: &MidiEventList) {
        for event in events.as_slice() {
            if let MidiEventKind::NoteOff { pitch } = event.kind {
                if self.started.get(pitch as usize).is_some_and(|&n| n > 0) {
                    self.keep_release(pitch);
                }
            }
        }
    }

    /// The kept NoteOffs, at offset 0 and before the block's own events.
    pub fn push_pending_releases(&mut self, list: &mut HostEventList) {
        for pitch in 0..PITCH_COUNT {
            if !self.release_pending[pitch] {
                continue;
            }
            if list.push(Event::note_off(0, pitch as u8, NO_NOTE_ID)) {
                self.release_pending[pitch] = false;
                self.started[pitch] = 0;
            }
        }
    }
}

impl Default for SoundingNotes {
    fn default() -> Self {
        Self::new()
    }
}

/// Append the engine's per-block `MidiEventList` as VST3 events: notes as note
/// events (velocity `0..127` → `0..1`), polyphonic aftertouch as poly pressure.
/// Other controls go through the controller's `IMidiMapping` (`midi_mapping`).
///
/// Pure — no FFI, no allocation.  A NoteOff the full list cannot take is kept
/// in `notes` for the next block.
pub fn push_vst3_events(
    events: &MidiEventList,
    list: &mut HostEventList,
    notes: &mut SoundingNotes,
) {
    for event in events.as_slice() {
        match event.kind {
            MidiEventKind::NoteOn { pitch, velocity } => {
                let note_on =
                    Event::note_on(event.offset, pitch, velocity as f32 / 127.0, NO_NOTE_ID);
                if list.push(note_on) {
                    notes.note_started(pitch);
                }
            }
            MidiEventKind::NoteOff { pitch } => {
                if list.push(Event::note_off(event.offset, pitch, NO_NOTE_ID)) {
                    notes.note_ended(pitch);
                } else {
                    notes.keep_release(pitch);
                }
            }
            MidiEventKind::Control { message } => {
                if message[0] & 0xF0 == MIDI_POLY_PRESSURE {
                    list.push(Event::poly_pressure(
                        event.offset,
                        message[1],
                        message[2] as f32 / 127.0,
                        NO_NOTE_ID,
                    ));
                }
            }
        }
    }
}

pub struct VstInstrument {
    /// Shared, refcounted handle to the live VST3 plugin (host owns a clone).
    instance: Arc<PluginInstance>,

    // Per-block scratch — allocated in `prepare`, stable thereafter.
    out_channel_ptrs: Vec<*mut f32>,
    out_bus: AudioBusBuffers,
    param_changes: Option<Box<HostParameterChanges>>,
    events: Option<Box<HostEventList>>,

    num_channels: usize,
    active: bool,
    notes: SoundingNotes,
    midi_map: MidiControllerMap,
    /// Set after the first SEH exception from `process()`: silent until the
    /// graph is next recompiled.
    crashed: bool,
    param_controls: PluginParamControls,
}

// SAFETY: identical discipline to `VstWrapperNode` — the COM pointers are
// reached through the shared `PluginInstance`, serially: `prepare` on the
// command thread, `process_midi` on the audio thread.
unsafe impl Send for VstInstrument {}

impl VstInstrument {
    /// Build from a shared VST3 [`PluginInstance`].  Caller guarantees the
    /// instance was created with VST3 native handles.
    pub fn new(instance: Arc<PluginInstance>) -> Self {
        debug_assert!(
            instance.vst3_handles().is_some(),
            "VstInstrument requires a VST3 PluginInstance"
        );
        VstInstrument {
            instance,
            out_channel_ptrs: Vec::new(),
            out_bus: unsafe { std::mem::zeroed() },
            param_changes: None,
            events: None,
            num_channels: 2,
            active: false,
            notes: SoundingNotes::new(),
            midi_map: MidiControllerMap::empty(),
            crashed: false,
            param_controls: PluginParamControls::default(),
        }
    }
}

impl InstrumentDevice for VstInstrument {
    fn prepare(&mut self, ctx: PrepareContext) {
        self.num_channels = ctx.num_channels;
        self.out_channel_ptrs = vec![std::ptr::null_mut(); ctx.num_channels];
        self.param_changes = Some(HostParameterChanges::boxed());
        self.events = Some(HostEventList::boxed());
        // Activation is owned by the shared instance (ADR-016); idempotent.
        self.active = self
            .instance
            .vst3_ensure_active(ctx.sample_rate, ctx.max_block_size as u32);

        // Skipped when a block is in flight: the controllers stay unmapped until
        // the next recompile.
        if let (Some(controller), Some(_gate)) = (
            self.instance.vst3_controller(),
            self.instance.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET),
        ) {
            self.midi_map = unsafe { read_midi_mapping(controller.as_ptr()) };
        }
    }

    fn process_midi(
        &mut self,
        events: &MidiEventList,
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        if self.crashed || !self.active || outputs.is_empty() {
            return;
        }
        // While an off-RT caller (state save, parameter read) holds the FFI gate
        // this block stays silent rather than race it.
        let Some(_ffi_gate) = self.instance.rt_try_enter() else {
            self.notes.keep_releases_of(events);
            return;
        };
        let (Some(event_list), Some(param_changes)) =
            (self.events.as_mut(), self.param_changes.as_mut())
        else {
            return;
        };

        event_list.reset();
        self.notes.push_pending_releases(event_list);
        push_vst3_events(events, event_list, &mut self.notes);
        fill_param_changes(&self.instance, &self.param_controls, param_changes);
        push_mapped_controls(events, &self.midi_map, param_changes);
        let param_changes_ptr = if param_changes.count > 0 {
            param_changes.as_mut() as *mut HostParameterChanges as *mut c_void
        } else {
            std::ptr::null_mut()
        };

        for (channel, slot) in self.out_channel_ptrs.iter_mut().enumerate() {
            *slot = outputs[0].channel_mut(channel).as_mut_ptr();
        }
        self.out_bus = AudioBusBuffers {
            num_channels: self.num_channels as i32,
            silence_flags: 0,
            channel_buffers32: self.out_channel_ptrs.as_mut_ptr(),
        };

        let mut context = vst3_process_context(transport);
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: transport.block_size as i32,
            num_inputs: 0,
            num_outputs: 1,
            inputs: std::ptr::null_mut(),
            outputs: &mut self.out_bus,
            input_param_changes: param_changes_ptr,
            output_param_changes: std::ptr::null_mut(),
            input_events: event_list.as_ffi_ptr(),
            output_events: std::ptr::null_mut(),
            process_context: &mut context as *mut ProcessContext as *mut c_void,
        };

        let (_, processor) = self
            .instance
            .vst3_handles()
            .expect("VstInstrument missing VST3 handles (programmer error)");
        let mut guarded = Vst3ProcessCtx {
            proc_ptr: processor.as_ptr(),
            data_ptr: &mut data,
        };
        let status = unsafe {
            crash_guard::guarded(
                vst3_process_trampoline,
                &mut guarded as *mut Vst3ProcessCtx as *mut c_void,
            )
        };
        if status == crash_guard::CAUGHT {
            self.crashed = true;
            // Not reused by the next compile: it gets a fresh instance.
            self.instance.set_lifecycle(LifecycleState::Error);
            crate::plugin_system::crash_marker::note_crash();
        }
    }

    fn inject_param_controls(&mut self, controls: Vec<PluginParamControl>) {
        track_value_modulation(&self.instance, &controls);
        self.param_controls = PluginParamControls::new(controls);
    }

    fn param_controls_mut(&mut self) -> Option<&mut PluginParamControls> {
        Some(&mut self.param_controls)
    }

    fn reset(&mut self) {
        self.notes.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::midi::MidiEvent;
    use crate::plugin_formats::vst3::ffi::{
        K_NOTE_OFF_EVENT, K_NOTE_ON_EVENT, K_POLY_PRESSURE_EVENT,
    };

    fn midi(offset: u32, kind: MidiEventKind) -> MidiEvent {
        MidiEvent { offset, kind }
    }

    fn list_of(events: &[MidiEvent]) -> MidiEventList {
        let mut list = MidiEventList::new();
        for &event in events {
            list.push(event);
        }
        list
    }

    /// `(kind, sample offset, pitch)` of every event in the list.
    fn summary(list: &HostEventList) -> Vec<(u16, i32, i16)> {
        list.as_slice()
            .iter()
            .map(|event| {
                let pitch = match event.kind {
                    K_NOTE_ON_EVENT => unsafe { event.payload.note_on.pitch },
                    K_NOTE_OFF_EVENT => unsafe { event.payload.note_off.pitch },
                    _ => unsafe { event.payload.poly_pressure.pitch },
                };
                (event.kind, event.sample_offset, pitch)
            })
            .collect()
    }

    fn convert(events: &MidiEventList, notes: &mut SoundingNotes) -> Box<HostEventList> {
        let mut list = HostEventList::boxed();
        notes.push_pending_releases(&mut list);
        push_vst3_events(events, &mut list, notes);
        list
    }

    #[test]
    fn notes_keep_their_order_offsets_and_scaled_velocity() {
        let events = list_of(&[
            midi(
                0,
                MidiEventKind::NoteOn {
                    pitch: 60,
                    velocity: 127,
                },
            ),
            midi(
                24,
                MidiEventKind::NoteOn {
                    pitch: 64,
                    velocity: 64,
                },
            ),
            midi(100, MidiEventKind::NoteOff { pitch: 60 }),
        ]);
        let mut notes = SoundingNotes::new();
        let list = convert(&events, &mut notes);

        assert_eq!(
            summary(&list),
            vec![
                (K_NOTE_ON_EVENT, 0, 60),
                (K_NOTE_ON_EVENT, 24, 64),
                (K_NOTE_OFF_EVENT, 100, 60),
            ]
        );
        let first = unsafe { list.as_slice()[0].payload.note_on };
        let second = unsafe { list.as_slice()[1].payload.note_on };
        assert_eq!((first.velocity, first.channel, first.note_id), (1.0, 0, -1));
        assert!((second.velocity - 64.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn polyphonic_aftertouch_becomes_poly_pressure() {
        let events = list_of(&[midi(
            5,
            MidiEventKind::Control {
                message: [0xA0, 62, 127],
            },
        )]);
        let list = convert(&events, &mut SoundingNotes::new());

        assert_eq!(summary(&list), vec![(K_POLY_PRESSURE_EVENT, 5, 62)]);
        let pressure = unsafe { list.as_slice()[0].payload.poly_pressure.pressure };
        assert_eq!(pressure, 1.0);
    }

    #[test]
    fn controls_are_not_sent_as_events() {
        let events = list_of(&[
            midi(
                0,
                MidiEventKind::Control {
                    message: [0xE0, 0, 0x60],
                },
            ),
            midi(
                0,
                MidiEventKind::Control {
                    message: [0xB0, 1, 64],
                },
            ),
            midi(
                0,
                MidiEventKind::Control {
                    message: [0xD0, 50, 0],
                },
            ),
        ]);
        let list = convert(&events, &mut SoundingNotes::new());
        assert!(list.as_slice().is_empty());
    }

    #[test]
    fn a_reset_releases_each_sounding_note_once() {
        let mut notes = SoundingNotes::new();
        let on = list_of(&[
            midi(
                0,
                MidiEventKind::NoteOn {
                    pitch: 60,
                    velocity: 100,
                },
            ),
            midi(
                0,
                MidiEventKind::NoteOn {
                    pitch: 67,
                    velocity: 100,
                },
            ),
            midi(10, MidiEventKind::NoteOff { pitch: 67 }),
        ]);
        convert(&on, &mut notes);

        notes.release_all();
        let next_block = convert(&MidiEventList::new(), &mut notes);
        assert_eq!(summary(&next_block), vec![(K_NOTE_OFF_EVENT, 0, 60)]);

        let after = convert(&MidiEventList::new(), &mut notes);
        assert!(after.as_slice().is_empty());
    }

    #[test]
    fn a_note_off_of_a_skipped_block_is_sent_with_the_next_one() {
        let mut notes = SoundingNotes::new();
        convert(
            &list_of(&[midi(
                0,
                MidiEventKind::NoteOn {
                    pitch: 60,
                    velocity: 100,
                },
            )]),
            &mut notes,
        );

        let skipped = list_of(&[midi(30, MidiEventKind::NoteOff { pitch: 60 })]);
        notes.keep_releases_of(&skipped);

        let next_block = convert(&MidiEventList::new(), &mut notes);
        assert_eq!(summary(&next_block), vec![(K_NOTE_OFF_EVENT, 0, 60)]);
    }

    #[test]
    fn a_skipped_note_off_for_a_note_the_plugin_never_heard_is_ignored() {
        let mut notes = SoundingNotes::new();
        notes.keep_releases_of(&list_of(&[midi(0, MidiEventKind::NoteOff { pitch: 60 })]));
        let list = convert(&MidiEventList::new(), &mut notes);
        assert!(list.as_slice().is_empty());
    }

    #[test]
    fn a_note_off_the_full_list_cannot_take_is_kept() {
        let mut notes = SoundingNotes::new();
        convert(
            &list_of(&[midi(
                0,
                MidiEventKind::NoteOn {
                    pitch: 60,
                    velocity: 100,
                },
            )]),
            &mut notes,
        );

        let mut full = HostEventList::boxed();
        while full.push(Event::note_on(0, 1, 1.0, -1)) {}
        push_vst3_events(
            &list_of(&[midi(8, MidiEventKind::NoteOff { pitch: 60 })]),
            &mut full,
            &mut notes,
        );

        let next_block = convert(&MidiEventList::new(), &mut notes);
        assert_eq!(summary(&next_block), vec![(K_NOTE_OFF_EVENT, 0, 60)]);
    }

    #[test]
    fn a_stray_note_off_does_not_underflow_the_count() {
        let mut notes = SoundingNotes::new();
        convert(
            &list_of(&[midi(0, MidiEventKind::NoteOff { pitch: 60 })]),
            &mut notes,
        );
        notes.release_all();
        let list = convert(&MidiEventList::new(), &mut notes);
        assert!(list.as_slice().is_empty());
    }
}
