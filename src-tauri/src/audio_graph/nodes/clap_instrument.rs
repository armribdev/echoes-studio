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
//! - `in_events` carries notes **and** parameter events (queued edits,
//!   modulation, automation — ADR-021) in one time-sorted list.

use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::{MidiEventKind, MidiEventList};
use crate::audio_graph::node::{PrepareContext, TransportInfo};
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::audio_graph::plugin_params::{PluginParamControl, PluginParamControls};
use crate::plugin_system::instance::{PluginInstance, FFI_GATE_HARVEST_BUDGET};

use crate::plugin_formats::clap::audio_ports::ClapOutputBuses;
use crate::plugin_formats::clap::ffi::{
    audio_thread_scope, ClapEventBuffer, ClapOutputEvents, ClapPlugin, ClapProcess,
    CLAP_PROCESS_ERROR,
};
use crate::plugin_formats::clap::params::{
    observe_output_event, read_clap_params, ClapParamEvents,
};
use crate::plugin_formats::clap::transport::clap_transport_event;

/// Append the engine's per-block `MidiEventList` as CLAP note events.
///
/// Pure — no FFI, no allocation (the list is pre-allocated and bounded).  Kept
/// separate from `process_midi` so it can be unit-tested without a live plugin.
/// Velocity maps from the MIDI 0..127 byte to CLAP's 0..1 `f64`.
pub fn push_clap_notes(events: &MidiEventList, list: &mut ClapEventBuffer) {
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

pub struct ClapInstrument {
    /// Shared, refcounted handle to the live CLAP plugin (host owns a clone).
    instance: Arc<PluginInstance>,

    // Per-block scratch — allocated in `prepare`, stable thereafter.
    /// Every declared output bus (ADR-014 multi-out: drum machines, samplers).
    outputs: ClapOutputBuses,
    /// Notes and parameter events for the block, sorted by time.
    param_events: ClapParamEvents,
    out_events: ClapOutputEvents,

    active: bool,
    /// Sample counter passed as `steady_time`. Must grow by `frames_count`
    /// every block so GUI notes (Vital's keyboard) land on a continuous clock.
    steady_time: i64,
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
        Self {
            instance,
            outputs: ClapOutputBuses::new(&[]),
            param_events: ClapParamEvents::new(),
            out_events: ClapOutputEvents {
                ctx: std::ptr::null_mut(),
                try_push: observe_output_event,
            },
            active: false,
            steady_time: 0,
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
    fn num_output_ports(&self) -> usize {
        self.outputs.num_ports()
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        // Activation lifecycle is owned by the shared instance (ADR-016) so it
        // survives a wrapper rebuild when the live instance is reused across a
        // recompile.  Idempotent: an already-active instance is left untouched.
        self.active = self
            .instance
            .clap_ensure_active(ctx.sample_rate, ctx.max_block_size as u32);

        self.outputs = ClapOutputBuses::new(self.instance.output_ports());
        self.outputs.prepare(ctx.num_channels, ctx.max_block_size);

        // Same fencing as the effect wrapper: skip if a block is in flight.
        if self.active {
            if let Some(_gate) = self.instance.offrt_try_acquire(FFI_GATE_HARVEST_BUDGET) {
                let params = unsafe { read_clap_params(self.plugin_ptr()) };
                self.param_events.set_ranges(&params);
            }
        }
    }

    fn process_midi(
        &mut self,
        events: &MidiEventList,
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        if outputs.is_empty() || !self.active {
            return;
        }
        let _audio_thread = audio_thread_scope();
        // Same FFI gate as the effect wrapper: while an off-RT caller (state
        // save, parameter read) holds it, this block stays silent rather than race it.
        let Some(_ffi_gate) = self.instance.rt_try_enter() else {
            self.steady_time = self.steady_time.wrapping_add(transport.block_size as i64);
            return;
        };
        self.instance.clap_start_processing();

        self.param_events.fill(&self.instance);
        push_clap_notes(events, &mut self.param_events.buffer);
        self.param_events.buffer.sort();
        self.out_events.ctx = self.param_events.output_context();

        // Frame count MUST be the current block size, not the buffer's allocated
        // capacity (`num_frames` = MAX_BLOCK_FRAMES).  Telling the plugin to
        // render the full 4096-frame capacity every callback advances its clock
        // far past the real block (e.g. 8× at a 512-frame device buffer), so the
        // synth races ahead and renders garbage that is mostly discarded — heard
        // as "no sound" / glitching.  Matches `PolySynth`, which uses block_size.
        let nf = transport.block_size;
        let (audio_outputs, audio_outputs_count) = self.outputs.bind(outputs);

        let transport_event = clap_transport_event(transport);
        let process = ClapProcess {
            steady_time: self.steady_time,
            frames_count: nf as u32,
            transport: &transport_event,
            audio_inputs: std::ptr::null(), // generator: no audio input bus
            audio_outputs,
            audio_inputs_count: 0,
            audio_outputs_count,
            in_events: &self.param_events.buffer.input_events_vtable,
            out_events: &self.out_events,
        };

        let p = self.plugin_ptr();
        let status = unsafe { ((*p).process)(p, &process) };
        self.steady_time = self.steady_time.wrapping_add(nf as i64);
        // On error an instrument has no input to pass through — leave the
        // scheduler-zeroed output as silence.
        let _ = status == CLAP_PROCESS_ERROR;
        self.outputs.finish(outputs);
    }

    fn inject_param_controls(&mut self, controls: Vec<PluginParamControl>) {
        self.param_events.set_controls(controls, &self.instance);
    }

    fn param_controls_mut(&mut self) -> Option<&mut PluginParamControls> {
        Some(self.param_events.controls_mut())
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

// NOTE: no `Drop` impl here (ADR-016).  The CLAP `stop_processing → deactivate
// → destroy` sequence is owned by `PluginInstance::Drop`.  A displaced wrapper
// must not deactivate the plugin, since a recompiled graph may reuse the same
// live instance.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::midi::MidiEvent;
    use crate::plugin_formats::clap::ffi::{
        ClapEventHeader, ClapEventNote, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON,
    };

    fn notes(list: &ClapEventBuffer) -> Vec<(u16, u32, i16, f64)> {
        (0..list.len())
            .map(|i| {
                let header = list.header(i).unwrap();
                let note = unsafe { &*(header as *const ClapEventHeader as *const ClapEventNote) };
                (header.r#type, header.time, note.key, note.velocity)
            })
            .collect()
    }

    #[test]
    fn push_clap_notes_maps_on_and_off() {
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

        let mut list = ClapEventBuffer::new();
        push_clap_notes(&events, &mut list);
        list.sort();

        assert_eq!(
            notes(&list),
            vec![
                (CLAP_EVENT_NOTE_ON, 24_000, 60, 1.0),
                (CLAP_EVENT_NOTE_OFF, 48_000, 60, 0.0),
            ]
        );
    }
}
