//! CLAP instrument adapter — drives a CLAP plugin as an `InstrumentDevice`.
//!
//! ## Role (Phase 5c, ADR-013)
//!
//! `InstrumentNode` already turns timeline MIDI clips into a per-block
//! `MidiEventList` with beat-accurate, tempo-live sample offsets (ADR-012).
//! Its inner generator is a `Box<dyn InstrumentDevice>` — the built-in
//! `PolySynth` by default.  `ClapInstrument` is an alternative inner generator
//! that forwards those notes to a CLAP plugin instance and renders its audio
//! output.  This is the clip-driven path to P0-2 ("Vital/Diva jouables") and
//! keeps the existing graph topology unchanged.
//!
//! ## Real-time contract
//!
//! Mirrors [`ClapWrapperNode`]: every `Vec` / `Box` is allocated in `prepare`;
//! `process_midi` only fills pre-allocated buffers and calls
//! `ClapPlugin::process` — no heap ops, no locks.  The plugin pointer is reached
//! through the `Arc<PluginInstance>` shared with the host (a non-atomic field
//! load via `Arc::deref`).
//!
//! ## Differences from the effect wrapper
//!
//! - `audio_inputs_count = 0` — an instrument is a pure generator.
//! - `in_events` carries **note** events (`FixedEventList`) instead of the
//!   parameter list.  Parameter automation for instruments (a merged
//!   note+param list) is a documented follow-up; the param ring is drained and
//!   discarded here so it cannot back up.

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::{MidiEventKind, MidiEventList};
use crate::audio_graph::node::TransportInfo;
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::plugin_system::instance::PluginInstance;

use crate::plugin_formats::clap::ffi::{
    ClapAudioBuffer, ClapEventHeader, ClapOutputEvents, ClapPlugin, ClapProcess, FixedEventList,
    CLAP_PROCESS_ERROR,
};

// ── No-op output events (the plugin may push events out; we ignore them) ───────
unsafe extern "C" fn out_try_push(_: *const ClapOutputEvents, _: *const ClapEventHeader) -> bool {
    true
}

/// Translate the engine's per-block `MidiEventList` into CLAP note events.
///
/// Pure — no FFI, no allocation (the list is pre-allocated and bounded).  Kept
/// separate from `process_midi` so it can be unit-tested without a live plugin.
/// Velocity maps from the MIDI 0..127 byte to CLAP's 0..1 `f64`.
pub fn fill_clap_notes(events: &MidiEventList, list: &mut FixedEventList) {
    list.clear();
    for ev in events.as_slice() {
        match ev.kind {
            MidiEventKind::NoteOn { pitch, velocity } => {
                list.push_note_on(ev.offset, 0, pitch as i16, velocity as f64 / 127.0);
            }
            MidiEventKind::NoteOff { pitch } => {
                list.push_note_off(ev.offset, 0, pitch as i16);
            }
        }
    }
}

// ── ClapInstrument ─────────────────────────────────────────────────────────────

pub struct ClapInstrument {
    /// Shared, refcounted handle to the live CLAP plugin (host owns a clone).
    instance: Arc<PluginInstance>,

    // Per-block scratch — allocated in `prepare`, stable thereafter.
    out_channel_ptrs: Vec<*mut f32>,
    out_audio_buf: ClapAudioBuffer,
    note_events: Box<FixedEventList>,
    out_events: ClapOutputEvents,

    num_channels: usize,
    active: bool,
}

// SAFETY: identical discipline to `ClapWrapperNode` — CLAP access is
// single-threaded and sequenced by the scheduler's per-step ownership; the raw
// scratch pointers are only dereferenced inside `process_midi` on the audio
// thread, which holds exclusive access.
unsafe impl Send for ClapInstrument {}

impl ClapInstrument {
    /// Build from a shared CLAP [`PluginInstance`].  Caller guarantees the
    /// instance was created with a CLAP native handle.
    pub fn new(instance: Arc<PluginInstance>) -> Self {
        debug_assert!(
            instance.clap_plugin().is_some(),
            "ClapInstrument requires a CLAP PluginInstance"
        );
        ClapInstrument {
            instance,
            out_channel_ptrs: Vec::new(),
            out_audio_buf: unsafe { std::mem::zeroed() },
            note_events: FixedEventList::new(),
            out_events: ClapOutputEvents {
                ctx: std::ptr::null_mut(),
                try_push: out_try_push,
            },
            num_channels: 2,
            active: false,
        }
    }

    #[inline(always)]
    fn plugin_ptr(&self) -> *const ClapPlugin {
        self.instance
            .clap_plugin()
            .expect("ClapInstrument missing CLAP plugin pointer (programmer error)")
    }
}

impl InstrumentDevice for ClapInstrument {
    fn prepare(&mut self, sample_rate: f32, max_block_size: usize, num_channels: usize) {
        self.num_channels = num_channels;
        self.out_channel_ptrs = vec![std::ptr::null_mut(); num_channels];

        let p = self.plugin_ptr();
        unsafe {
            ((*p).init)(p);
            let ok = ((*p).activate)(p, sample_rate as f64, 1, max_block_size as u32);
            self.active = ok;
            if ok {
                ((*p).start_processing)(p);
            }
        }
    }

    fn process_midi(
        &mut self,
        events: &MidiEventList,
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        if outputs.is_empty() || !self.active {
            return;
        }

        // Instrument param automation (a merged note+param input list) is a
        // follow-up; drain-and-discard so the producer's ring never backs up.
        self.instance.drain_params(|_| {});

        // Notes → CLAP events (pre-allocated list, no alloc).
        fill_clap_notes(events, &mut self.note_events);

        let nc = self.num_channels;
        let nf = outputs[0].num_frames;
        for c in 0..nc {
            self.out_channel_ptrs[c] = outputs[0].channel_mut(c).as_mut_ptr();
        }

        self.out_audio_buf = ClapAudioBuffer {
            data32: self.out_channel_ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: nc as u32,
            latency: 0,
            constant_mask: 0,
        };

        let process = ClapProcess {
            steady_time: -1,
            frames_count: nf as u32,
            transport: std::ptr::null(),
            audio_inputs: std::ptr::null(), // generator: no audio input bus
            audio_outputs: &mut self.out_audio_buf,
            audio_inputs_count: 0,
            audio_outputs_count: 1,
            in_events: &self.note_events.input_events_vtable,
            out_events: &self.out_events,
        };

        let p = self.plugin_ptr();
        let status = unsafe { ((*p).process)(p, &process) };
        // On error an instrument has no input to pass through — leave the
        // scheduler-zeroed output as silence.
        let _ = status == CLAP_PROCESS_ERROR;
    }

    fn reset(&mut self) {
        if self.active {
            let p = self.plugin_ptr();
            unsafe {
                ((*p).reset)(p);
            }
        }
    }
}

impl Drop for ClapInstrument {
    fn drop(&mut self) {
        // Mirror `ClapWrapperNode`: stop_processing + deactivate before the
        // shared `Arc<PluginInstance>` releases (destroy fires when the last
        // clone drops).  Runs off-RT via the snapshot drop ring.
        if self.active {
            let p = self.plugin_ptr();
            unsafe {
                ((*p).stop_processing)(p);
                ((*p).deactivate)(p);
            }
            self.active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::midi::MidiEvent;

    #[test]
    fn fill_clap_notes_maps_on_and_off() {
        let mut events = MidiEventList::new();
        events.push(MidiEvent {
            offset: 24_000,
            kind: MidiEventKind::NoteOn {
                pitch: 60,
                velocity: 127,
            },
        });
        events.push(MidiEvent {
            offset: 48_000,
            kind: MidiEventKind::NoteOff { pitch: 60 },
        });

        let mut list = FixedEventList::new();
        fill_clap_notes(&events, &mut list);

        assert_eq!(list.count, 2);
        // NoteOn: full velocity → 1.0, key 60, time 24_000.
        assert_eq!(list.events[0].header.time, 24_000);
        assert_eq!(list.events[0].key, 60);
        assert!((list.events[0].velocity - 1.0).abs() < 1e-9);
        // NoteOff: time 48_000, key 60.
        assert_eq!(list.events[1].header.time, 48_000);
        assert_eq!(list.events[1].key, 60);
    }

    #[test]
    fn fill_clap_notes_clears_previous_block() {
        let mut list = FixedEventList::new();
        let mut a = MidiEventList::new();
        a.push(MidiEvent {
            offset: 0,
            kind: MidiEventKind::NoteOn {
                pitch: 64,
                velocity: 64,
            },
        });
        fill_clap_notes(&a, &mut list);
        assert_eq!(list.count, 1);
        // A subsequent empty block must reset the list.
        let empty = MidiEventList::new();
        fill_clap_notes(&empty, &mut list);
        assert_eq!(list.count, 0);
    }
}
