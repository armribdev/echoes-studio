//! Graph devices of a sandboxed CLAP plugin (ADR-015): [`BridgedNode`] for an
//! effect, [`BridgedInstrument`] for a track instrument.
//!
//! Same role as `ClapWrapperNode` / `ClapInstrument`, with the plugin in a
//! `plugin_host_bridge` process: each block, the device serialises the
//! transport, the parameter events (queued edits, automation, modulation — the
//! same `ClapParamEvents` as in-process) and, for an instrument, the notes; hands
//! them and the audio to the bridge; and replays the plugin's output events into
//! the same observers.
//!
//! ## Real-time behaviour
//!
//! The round-trip is synchronous: the audio thread spins, without allocating
//! or locking, until the bridge answers.  Live, the wait is bounded by 3/4 of
//! the block period; a late block is silence and eight in a row condemn the
//! plugin (it then stays silent at no cost).  An offline export waits up to
//! [`OFFLINE_WAIT`] per block so the render does not depend on scheduling.

use std::sync::Arc;
use std::time::Duration;

use plugin_bridge::protocol::{BLOCK_PLUGIN_EDITED, BLOCK_RESTART_REQUESTED};
use plugin_bridge::session::{BlockOutcome, BlockRequest};

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::MidiEventList;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::audio_graph::nodes::clap_instrument::push_clap_notes;
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::audio_graph::plugin_params::{parse_plugin_param_name, PluginParamControl, PluginParamControls};
use crate::plugin_formats::clap::ffi::{
    append_event, mark_plugin_state_edited, split_events, ClapEventHeader, ClapOutputEvents,
};
use crate::plugin_formats::clap::params::{observe_output_event, ClapParamEvents};
use crate::plugin_formats::clap::transport::clap_transport_event;
use crate::plugin_system::bridge::{CHANNELS, EVENT_CAPACITY, MAX_FRAMES};
use crate::plugin_system::crash_marker::note_crash;
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::lifecycle::LifecycleState;

/// Share of the block period the audio thread may wait for the bridge.
const LIVE_WAIT_SHARE: f64 = 0.75;
/// Per-block wait during an offline export: a plugin slower than this is hung.
const OFFLINE_WAIT: Duration = Duration::from_secs(10);
/// Largest plugin output event the device replays (CLAP core events are ≤ 56 B).
const MAX_OUTPUT_EVENT_BYTES: usize = 256;

/// One block's exchange with the bridge, shared by the effect and the instrument.
struct BridgeRoundTrip {
    instance: Arc<PluginInstance>,
    param_events: ClapParamEvents,
    active: bool,
    live: bool,
    sample_rate: f32,
    latency: u32,
    sequence: u64,
    steady_time: i64,
    pending_reset: bool,
    input: Vec<f32>,
    output: Vec<f32>,
    events: Vec<u8>,
    out_events: Vec<u8>,
}

// SAFETY: the raw pointers are the self-referential vtable contexts of the
// device's own event buffers; the device moves between threads as a whole.
unsafe impl Send for BridgeRoundTrip {}

impl BridgeRoundTrip {
    fn new(instance: Arc<PluginInstance>) -> Self {
        debug_assert!(instance.is_sandboxed(), "a bridged device requires a sandboxed instance");
        BridgeRoundTrip {
            instance,
            param_events: ClapParamEvents::new(),
            active: false,
            live: true,
            sample_rate: 48_000.0,
            latency: 0,
            sequence: 0,
            steady_time: 0,
            pending_reset: false,
            input: Vec::new(),
            output: Vec::new(),
            events: Vec::new(),
            out_events: Vec::new(),
        }
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.live = ctx.live;
        self.sample_rate = ctx.sample_rate;
        let max_frames = (ctx.max_block_size as u32).min(MAX_FRAMES);
        let samples = max_frames as usize * CHANNELS as usize;
        self.input = vec![0.0; samples];
        self.output = vec![0.0; samples];
        self.events = vec![0; EVENT_CAPACITY as usize];
        self.out_events = vec![0; EVENT_CAPACITY as usize];

        let bridge = self.instance.bridge().filter(|_| ctx.num_channels == CHANNELS as usize);
        let latency = bridge.and_then(|bridge| bridge.activate(ctx.sample_rate, max_frames));
        self.active = latency.is_some();
        self.latency = latency.unwrap_or(0);
        if let Some(bridge) = bridge.filter(|_| self.active) {
            self.param_events.set_ranges(&bridge.params());
        }
        self.instance.set_lifecycle(LifecycleState::Running);
    }

    fn wait_budget(&self, frames: usize) -> Duration {
        if self.live {
            Duration::from_secs_f64(frames as f64 / self.sample_rate as f64 * LIVE_WAIT_SHARE)
        } else {
            OFFLINE_WAIT
        }
    }

    /// Transport first, then the parameter events and notes sorted by time.
    /// `None` if they do not fit.
    fn write_events(&mut self, transport: &TransportInfo, notes: Option<&MidiEventList>) -> Option<usize> {
        let transport_event = clap_transport_event(transport);
        let end = append_event(&mut self.events, 0, &transport_event.header)?;
        self.param_events.fill(&self.instance);
        if let Some(notes) = notes {
            push_clap_notes(notes, &mut self.param_events.buffer);
        }
        self.param_events.buffer.sort();
        self.param_events.buffer.write_bytes(&mut self.events, end)
    }

    fn replay_output_events(&mut self, byte_count: usize) {
        let output = ClapOutputEvents {
            ctx: self.param_events.output_context(),
            try_push: observe_output_event,
        };
        let mut aligned = [0u64; MAX_OUTPUT_EVENT_BYTES / 8];
        for event in split_events(&self.out_events[..byte_count]) {
            let Some(bytes) = as_bytes_mut(&mut aligned).get_mut(..event.len()) else {
                continue;
            };
            bytes.copy_from_slice(event);
            // SAFETY: `aligned` holds a complete, 8-byte-aligned event.
            unsafe { observe_output_event(&output, aligned.as_ptr() as *const ClapEventHeader) };
        }
    }

    /// Runs one block through the bridge.  `false` when the block was not sent
    /// (plugin inactive, block too large, too many events, port busy): `output`
    /// is then untouched and the caller decides what to play.
    fn process(
        &mut self,
        input: Option<&AudioBuffer>,
        output: &mut AudioBuffer,
        transport: &TransportInfo,
        notes: Option<&MidiEventList>,
    ) -> bool {
        let frames = transport.block_size;
        let samples = frames * CHANNELS as usize;
        if !self.active || samples > self.output.len() {
            return false;
        }
        let Some(event_bytes) = self.write_events(transport, notes) else {
            return false;
        };
        if let Some(input) = input {
            for (channel, planar) in self.input.chunks_exact_mut(frames).take(CHANNELS as usize).enumerate() {
                planar.copy_from_slice(&input.channel(channel)[..frames]);
            }
        }

        let budget = self.wait_budget(frames);
        let Some(bridge) = self.instance.bridge() else {
            return false;
        };
        let request = BlockRequest {
            frames,
            input: input.is_some().then_some(&self.input[..samples]),
            events: &self.events[..event_bytes],
            sequence: self.sequence,
            steady_time: self.steady_time,
            reset: self.pending_reset,
        };
        let outcome = bridge.audio().process_block(&request, &mut self.output, &mut self.out_events, budget);
        let condemned = outcome == BlockOutcome::TimedOut && bridge.is_dead();
        self.sequence += 1;
        self.steady_time += frames as i64;

        match outcome {
            BlockOutcome::Skipped => return false,
            BlockOutcome::Processed { flags, event_bytes } => {
                self.pending_reset = false;
                self.replay_output_events(event_bytes);
                if flags & BLOCK_PLUGIN_EDITED != 0 {
                    mark_plugin_state_edited();
                }
                if flags & BLOCK_RESTART_REQUESTED != 0 {
                    if let Some(bridge) = self.instance.bridge() {
                        bridge.request_restart();
                    }
                }
            }
            BlockOutcome::TimedOut if condemned => {
                self.instance.set_lifecycle(LifecycleState::Error);
                note_crash();
            }
            BlockOutcome::TimedOut | BlockOutcome::Dead => {}
        }
        for (channel, planar) in self.output.chunks_exact(frames).take(CHANNELS as usize).enumerate() {
            output.channel_mut(channel)[..frames].copy_from_slice(planar);
        }
        true
    }

    fn set_param_controls(&mut self, controls: Vec<PluginParamControl>) {
        self.param_events.set_controls(controls, &self.instance);
    }

    fn param_controls(&mut self) -> &mut PluginParamControls {
        self.param_events.controls_mut()
    }
}

fn as_bytes_mut(words: &mut [u64]) -> &mut [u8] {
    // SAFETY: any bit pattern is a valid `u8`; the length covers the same bytes.
    unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, words.len() * 8) }
}

fn pass_through(inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer]) {
    if let (Some(input), Some(output)) = (inputs.first(), outputs.first_mut()) {
        output.copy_from(input);
    }
}

/// A sandboxed effect: a block that cannot be sent passes the input through.
pub struct BridgedNode {
    node_id: NodeId,
    round_trip: BridgeRoundTrip,
}

impl BridgedNode {
    /// `instance` must be sandboxed (`PluginInstance::new_bridged`).
    pub fn new(node_id: NodeId, instance: Arc<PluginInstance>) -> Self {
        BridgedNode {
            node_id,
            round_trip: BridgeRoundTrip::new(instance),
        }
    }
}

impl AudioNode for BridgedNode {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn label(&self) -> &str {
        "BridgedPlugin"
    }

    fn num_input_ports(&self) -> usize {
        1
    }

    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.round_trip.prepare(ctx);
    }

    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], transport: &TransportInfo) {
        let Some(output) = outputs.first_mut() else {
            return;
        };
        if !self.round_trip.process(inputs.first().copied(), output, transport, None) {
            pass_through(inputs, outputs);
        }
    }

    fn reset(&mut self) {
        self.round_trip.pending_reset = true;
    }

    fn param_id(&self, name: &str) -> Option<u32> {
        parse_plugin_param_name(name)
    }

    fn reset_modulation(&mut self) {
        self.round_trip.param_controls().clear_block();
    }

    fn apply_modulation(&mut self, param_id: u32, delta: f32) {
        self.round_trip.param_controls().modulate(param_id, delta);
    }

    fn apply_modulation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        self.round_trip.param_controls().modulate_at(param_id, sample_offset, value);
    }

    fn apply_automation(&mut self, param_id: u32, value: f32) {
        self.round_trip.param_controls().automate(param_id, value);
    }

    fn apply_automation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        self.round_trip.param_controls().automate_at(param_id, sample_offset, value);
    }

    fn inject_plugin_param_controls(&mut self, controls: Vec<PluginParamControl>) {
        self.round_trip.set_param_controls(controls);
    }

    fn latency_samples(&self) -> u32 {
        self.round_trip.latency
    }
}

/// A sandboxed track instrument: no audio input, the notes travel with the
/// parameter events.  A block that cannot be sent stays silent and its notes
/// are dropped, as with the in-process `ClapInstrument`.
pub struct BridgedInstrument {
    round_trip: BridgeRoundTrip,
}

impl BridgedInstrument {
    /// `instance` must be sandboxed (`PluginInstance::new_bridged`).
    pub fn new(instance: Arc<PluginInstance>) -> Self {
        BridgedInstrument {
            round_trip: BridgeRoundTrip::new(instance),
        }
    }
}

impl InstrumentDevice for BridgedInstrument {
    fn prepare(&mut self, ctx: PrepareContext) {
        self.round_trip.prepare(ctx);
    }

    fn process_midi(&mut self, events: &MidiEventList, outputs: &mut [AudioBuffer], transport: &TransportInfo) {
        if let Some(output) = outputs.first_mut() {
            self.round_trip.process(None, output, transport, Some(events));
        }
    }

    fn reset(&mut self) {
        self.round_trip.pending_reset = true;
    }

    fn inject_param_controls(&mut self, controls: Vec<PluginParamControl>) {
        self.round_trip.set_param_controls(controls);
    }

    fn param_controls_mut(&mut self) -> Option<&mut PluginParamControls> {
        Some(self.round_trip.param_controls())
    }
}
