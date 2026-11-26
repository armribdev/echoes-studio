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
use crate::plugin_system::param_edits::EditCapture;

use super::buses::Vst3OutputBuses;
use super::event_list::HostEventList;
use super::ffi::{HostParameterChanges, ProcessContext, ProcessData};
use super::midi_mapping::{push_mapped_controls, read_midi_mapping, MidiControllerMap};
use super::notes::{push_events, NoteKind, NoteMessage, SoundingNotes};
use super::param_delivery::{
    begin_output_changes, fill_param_changes, publish_output_changes, track_value_modulation,
};
use super::transport::vst3_process_context;
use super::wrapper::{vst3_process_trampoline, Vst3ProcessCtx};

/// The engine's per-block notes and controls as the messages `vst3_host::notes`
/// turns into VST3 events, in the same order.
fn note_messages(events: &MidiEventList) -> impl Iterator<Item = NoteMessage> + '_ {
    events.as_slice().iter().map(|event| NoteMessage {
        offset: event.offset,
        kind: match event.kind {
            MidiEventKind::NoteOn { pitch, velocity } => NoteKind::NoteOn { pitch, velocity },
            MidiEventKind::NoteOff { pitch } => NoteKind::NoteOff { pitch },
            MidiEventKind::Control { message } => NoteKind::Control { message },
        },
    })
}

pub struct VstInstrument {
    /// Shared, refcounted handle to the live VST3 plugin (host owns a clone).
    instance: Arc<PluginInstance>,

    // Per-block scratch — allocated in `prepare`, stable thereafter.
    /// One bus per graph output port (ADR-014 multi-out).
    outputs: Vst3OutputBuses,
    param_changes: Option<Box<HostParameterChanges>>,
    /// Where the plugin writes the changes it makes itself, for its controller.
    output_changes: Option<Box<HostParameterChanges>>,
    events: Option<Box<HostEventList>>,

    num_channels: usize,
    active: bool,
    /// Read at each `prepare`, after the activation it belongs to.
    reported_latency: u32,
    notes: SoundingNotes,
    midi_map: MidiControllerMap,
    /// Set after the first SEH exception from `process()`: silent until the
    /// graph is next recompiled.
    crashed: bool,
    param_controls: PluginParamControls,

    /// What the user drags in the plugin window, for automation writing (ADR-037).
    edit_capture: EditCapture,
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
            edit_capture: EditCapture::for_instance(&instance),
            instance,
            outputs: Vst3OutputBuses::new(&[]),
            param_changes: None,
            output_changes: None,
            events: None,
            num_channels: 2,
            active: false,
            reported_latency: 0,
            notes: SoundingNotes::new(),
            midi_map: MidiControllerMap::empty(),
            crashed: false,
            param_controls: PluginParamControls::default(),
        }
    }
}

impl InstrumentDevice for VstInstrument {
    fn num_output_ports(&self) -> usize {
        self.outputs.num_ports()
    }

    fn latency_samples(&self) -> u32 {
        self.reported_latency
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.num_channels = ctx.num_channels;
        self.param_changes = Some(HostParameterChanges::boxed());
        self.output_changes = Some(HostParameterChanges::boxed());
        self.events = Some(HostEventList::boxed());
        // Activation is owned by the shared instance (ADR-016); idempotent.
        self.active = self
            .instance
            .vst3_ensure_active(ctx.sample_rate, ctx.max_block_size as u32);
        self.outputs = Vst3OutputBuses::new(&self.instance.vst3_output_buses());
        self.outputs.prepare(ctx.num_channels, ctx.max_block_size);

        if let Some(latency) = self.instance.vst3_latency_samples() {
            self.reported_latency = latency;
        }

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
            self.notes.keep_releases_of(note_messages(events));
            return;
        };
        let (Some(event_list), Some(param_changes)) =
            (self.events.as_mut(), self.param_changes.as_mut())
        else {
            return;
        };

        event_list.reset();
        self.notes.push_pending_releases(event_list);
        push_events(note_messages(events), event_list, &mut self.notes);
        self.edit_capture.begin_block(transport);
        fill_param_changes(
            &self.instance,
            &self.param_controls,
            &mut self.edit_capture,
            param_changes,
        );
        let controls = events
            .as_slice()
            .iter()
            .filter_map(|event| match event.kind {
                MidiEventKind::Control { message } => Some((event.offset, message)),
                _ => None,
            });
        push_mapped_controls(controls, &self.midi_map, param_changes);
        let param_changes_ptr = if param_changes.count > 0 {
            param_changes.as_mut() as *mut HostParameterChanges as *mut c_void
        } else {
            std::ptr::null_mut()
        };

        let (output_buses, output_bus_count) = self.outputs.bind(outputs);

        let output_changes_ptr = self
            .output_changes
            .as_mut()
            .map_or(std::ptr::null_mut(), |changes| {
                begin_output_changes(changes)
            });

        let mut context = vst3_process_context(transport);
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: transport.block_size as i32,
            num_inputs: 0,
            num_outputs: output_bus_count,
            inputs: std::ptr::null_mut(),
            outputs: output_buses,
            input_param_changes: param_changes_ptr,
            output_param_changes: output_changes_ptr,
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
        } else {
            self.outputs.finish(outputs);
            if let Some(changes) = self.output_changes.as_ref() {
                publish_output_changes(&self.instance, changes);
            }
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

    #[test]
    fn the_engine_events_become_messages_in_the_same_order() {
        let mut list = MidiEventList::new();
        for (offset, kind) in [
            (
                3,
                MidiEventKind::NoteOn {
                    pitch: 60,
                    velocity: 100,
                },
            ),
            (
                3,
                MidiEventKind::Control {
                    message: [0xE0, 1, 2],
                },
            ),
            (9, MidiEventKind::NoteOff { pitch: 60 }),
        ] {
            list.push(MidiEvent { offset, kind });
        }
        let messages: Vec<NoteMessage> = note_messages(&list).collect();
        assert_eq!(
            messages,
            [
                NoteMessage {
                    offset: 3,
                    kind: NoteKind::NoteOn {
                        pitch: 60,
                        velocity: 100
                    }
                },
                NoteMessage {
                    offset: 3,
                    kind: NoteKind::Control {
                        message: [0xE0, 1, 2]
                    }
                },
                NoteMessage {
                    offset: 9,
                    kind: NoteKind::NoteOff { pitch: 60 }
                },
            ]
        );
    }
}
