//! Graph devices of a sandboxed plugin (ADR-015, ADR-033): [`BridgedNode`] for an
//! effect, [`BridgedInstrument`] for a CLAP or VST3 track instrument.
//!
//! Same role as `ClapWrapperNode` / `ClapInstrument`, with the plugin in a
//! `plugin_host_bridge` process: each block, the device writes the transport,
//! the parameter events (queued edits, automation, modulation — the same
//! `ClapParamEvents` logic as in-process) and, for an instrument, the notes, as
//! format-neutral sections (protocol v3); hands them and the audio to the bridge,
//! whose adapter builds the CLAP events or the VST3 `ProcessData`; and replays the
//! plugin's edits into the same observers.  No plugin structure crosses the
//! process boundary.
//!
//! The parameter events follow the plugin's format: CLAP plain values and
//! modulation offsets, VST3 normalised values with the modulation added to a base
//! (ADR-021).  Both travel as the same wire records; the bridge converts nothing.
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

use plugin_bridge::protocol::{
    WireNote, WireParam, WireTransport, BLOCK_NOTE_NAMES_CHANGED, BLOCK_PARAMS_RESCAN_MASK,
    BLOCK_PARAMS_RESCAN_SHIFT, BLOCK_PLUGIN_EDITED, BLOCK_RESTART_REQUESTED, EXPRESSION_PITCH,
    EXPRESSION_PRESSURE, EXPRESSION_TIMBRE, MIDI, NOTE_OFF, NOTE_ON, PARAMS_INFO, PARAMS_LIST,
    PARAMS_TEXT, PARAMS_VALUES, PARAM_MODULATION, PARAM_VALUE, TRANSPORT_PLAYING,
};
use plugin_bridge::session::{BlockOutcome, BlockReply, BlockRequest};

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::midi::{MidiEventKind, MidiEventList};
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};
use crate::audio_graph::nodes::instrument::InstrumentDevice;
use crate::audio_graph::plugin_params::{
    parse_plugin_param_name, PluginParamControl, PluginParamControls,
};
use crate::plugin_formats::clap::ffi::{
    mark_plugin_note_names_changed, mark_plugin_params_rescan, mark_plugin_state_edited,
    CLAP_PARAM_RESCAN_ALL, CLAP_PARAM_RESCAN_INFO, CLAP_PARAM_RESCAN_TEXT,
    CLAP_PARAM_RESCAN_VALUES, MAX_EVENTS, MAX_PARAM_EVENTS,
};
use crate::plugin_formats::clap::params::{ClapParamEvents, ParamEventSink};
use crate::plugin_formats::vst3::param_delivery::{Vst3ParamEvents, Vst3ParamSink};
use crate::project::NoteExpressionKind;
use plugin_bridge::records::PORT_CHANNELS;

use crate::plugin_system::bridge::BridgedPlugin;
use crate::plugin_system::bridge::MAX_FRAMES;
use crate::plugin_system::crash_marker::note_crash;
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::wrapper::DeviceFormat;

/// Share of the block period the audio thread may wait for the bridge.
const LIVE_WAIT_SHARE: f64 = 0.75;
/// Per-block wait during an offline export: a plugin slower than this is hung.
const OFFLINE_WAIT: Duration = Duration::from_secs(10);
/// Edits and MIDI messages the device keeps from one reply; more are dropped, as
/// the bridge drops what does not fit its half of the event capacity.
const MAX_REPLY_EDITS: usize = 256;
const MAX_REPLY_MIDI: usize = 64;

/// The block's parameter events, as wire records.  Full means dropped, like the
/// CLAP event pool the same logic fills in the Echoes process.
struct WireParamSink {
    events: Vec<WireParam>,
}

impl WireParamSink {
    fn new() -> Self {
        WireParamSink {
            events: Vec::with_capacity(MAX_PARAM_EVENTS),
        }
    }

    fn push(&mut self, kind: u32, offset: u32, native_id: u32, value: f64) {
        if self.events.len() < MAX_PARAM_EVENTS {
            self.events
                .push(WireParam::new(kind, offset, native_id, value));
        }
    }
}

impl ParamEventSink for WireParamSink {
    fn push_value(&mut self, offset: u32, native_id: u32, plain: f64) {
        self.push(PARAM_VALUE, offset, native_id, plain);
    }

    fn push_modulation(&mut self, offset: u32, native_id: u32, amount: f64) {
        self.push(PARAM_MODULATION, offset, native_id, amount);
    }
}

impl Vst3ParamSink for WireParamSink {
    fn push_value(&mut self, offset: u32, native_id: u32, normalised: f64) {
        self.push(PARAM_VALUE, offset, native_id, normalised);
    }
}

/// The parameter logic of the plugin's format.
enum BridgedParams {
    Clap(ClapParamEvents),
    Vst3(Vst3ParamEvents),
}

impl BridgedParams {
    fn new(format: DeviceFormat) -> Self {
        match format {
            DeviceFormat::Vst3 => Self::Vst3(Vst3ParamEvents::new()),
            _ => Self::Clap(ClapParamEvents::new()),
        }
    }

    /// Off the audio thread: a CLAP plugin's plain ranges; VST3 values are
    /// normalised, so there is nothing to read.
    fn read_ranges(&mut self, bridge: &BridgedPlugin) {
        if let Self::Clap(events) = self {
            events.set_ranges(&bridge.params());
        }
    }

    fn set_controls(&mut self, controls: Vec<PluginParamControl>, instance: &Arc<PluginInstance>) {
        match self {
            Self::Clap(events) => events.set_controls(controls, instance),
            Self::Vst3(events) => events.set_controls(controls, instance),
        }
    }

    fn controls_mut(&mut self) -> &mut PluginParamControls {
        match self {
            Self::Clap(events) => events.controls_mut(),
            Self::Vst3(events) => events.controls_mut(),
        }
    }

    fn fill_into(&mut self, instance: &PluginInstance, sink: &mut WireParamSink) {
        match self {
            Self::Clap(events) => events.fill_into(instance, sink),
            Self::Vst3(events) => events.fill_into(instance, sink),
        }
    }

    /// Where the block sits on the timeline, for the edits its reply carries (ADR-037).
    fn begin_block(&mut self, transport: &TransportInfo) {
        match self {
            Self::Clap(events) => events.begin_block(transport),
            Self::Vst3(events) => events.begin_block(transport),
        }
    }

    fn observe_edit(&mut self, edit: &WireParam) {
        match self {
            Self::Clap(events) => events.observe_edit(edit),
            Self::Vst3(events) => events.observe_edit(edit),
        }
    }
}

fn wire_transport(transport: &TransportInfo) -> WireTransport {
    let position = transport.musical_position();
    WireTransport {
        flags: if transport.is_playing {
            TRANSPORT_PLAYING
        } else {
            0
        },
        bar_number: position.bar_number,
        sample_rate: transport.sample_rate as f64,
        position_samples: transport.position_samples as i64,
        position_seconds: transport.position_seconds,
        position_beats: position.beats,
        bar_start_beats: position.bar_start_beats,
        tempo: transport.bpm as f64,
        time_signature_numerator: transport.time_signature.numerator as u16,
        time_signature_denominator: transport.time_signature.denominator as u16,
        reserved: 0,
    }
}

/// The engine's notes and controls as wire notes, in the same order.  The vector
/// never grows: what does not fit its capacity is dropped.
fn push_wire_notes(events: &MidiEventList, notes: &mut Vec<WireNote>) {
    for event in events.as_slice() {
        if notes.len() == notes.capacity() {
            break;
        }
        let note = match event.kind {
            MidiEventKind::NoteOn { pitch, velocity } => {
                WireNote::new(event.offset, NOTE_ON, [pitch, velocity, 0])
            }
            MidiEventKind::NoteOff { pitch } => {
                WireNote::new(event.offset, NOTE_OFF, [pitch, 0, 0])
            }
            MidiEventKind::Control { message } => WireNote::new(event.offset, MIDI, message),
            MidiEventKind::NoteExpression { pitch, kind, value } => {
                WireNote::expression(event.offset, pitch, wire_expression(kind), value)
            }
        };
        notes.push(note);
    }
}

fn wire_expression(kind: NoteExpressionKind) -> u8 {
    match kind {
        NoteExpressionKind::Pressure => EXPRESSION_PRESSURE,
        NoteExpressionKind::Timbre => EXPRESSION_TIMBRE,
        NoteExpressionKind::Pitch => EXPRESSION_PITCH,
    }
}

/// The `CLAP_PARAM_RESCAN_*` flags the app reads, for the protocol's `PARAMS_*` bits.
fn rescan_flags(changes: u32) -> u32 {
    [
        (PARAMS_VALUES, CLAP_PARAM_RESCAN_VALUES),
        (PARAMS_TEXT, CLAP_PARAM_RESCAN_TEXT),
        (PARAMS_INFO, CLAP_PARAM_RESCAN_INFO),
        (PARAMS_LIST, CLAP_PARAM_RESCAN_ALL),
    ]
    .into_iter()
    .filter(|(neutral, _)| changes & neutral != 0)
    .fold(0, |flags, (_, clap)| flags | clap)
}

/// One block's exchange with the bridge, shared by the effect and the instrument.
struct BridgeRoundTrip {
    instance: Arc<PluginInstance>,
    param_events: BridgedParams,
    active: bool,
    live: bool,
    sample_rate: f32,
    latency: u32,
    /// Channels of the input and of the output samples of a block, read from the
    /// bridge's port table when the device is prepared.
    input_channels: usize,
    output_channels: usize,
    sequence: u64,
    steady_time: i64,
    pending_reset: bool,
    input: Vec<f32>,
    output: Vec<f32>,
    wire_params: WireParamSink,
    wire_notes: Vec<WireNote>,
    edits: Vec<WireParam>,
    midi_out: Vec<WireNote>,
}

// SAFETY: the raw pointers are the self-referential vtable contexts of the
// device's own event buffers; the device moves between threads as a whole.
unsafe impl Send for BridgeRoundTrip {}

impl BridgeRoundTrip {
    fn new(instance: Arc<PluginInstance>) -> Self {
        debug_assert!(
            instance.is_sandboxed(),
            "a bridged device requires a sandboxed instance"
        );
        BridgeRoundTrip {
            param_events: BridgedParams::new(instance.format),
            instance,
            active: false,
            live: true,
            sample_rate: 48_000.0,
            latency: 0,
            input_channels: PORT_CHANNELS,
            output_channels: PORT_CHANNELS,
            sequence: 0,
            steady_time: 0,
            pending_reset: false,
            input: Vec::new(),
            output: Vec::new(),
            wire_params: WireParamSink::new(),
            wire_notes: Vec::with_capacity(MAX_EVENTS),
            edits: vec![WireParam::new(PARAM_VALUE, 0, 0, 0.0); MAX_REPLY_EDITS],
            midi_out: vec![WireNote::new(0, MIDI, [0; 3]); MAX_REPLY_MIDI],
        }
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.live = ctx.live;
        self.sample_rate = ctx.sample_rate;
        let max_frames = (ctx.max_block_size as u32).min(MAX_FRAMES);
        let bridge = self
            .instance
            .bridge()
            .filter(|_| ctx.num_channels == PORT_CHANNELS);
        self.input_channels = bridge.map_or(PORT_CHANNELS, BridgedPlugin::input_channels);
        self.output_channels = bridge.map_or(PORT_CHANNELS, |bridge| {
            bridge.output_ports() * PORT_CHANNELS
        });
        self.input = vec![0.0; max_frames as usize * self.input_channels];
        self.output = vec![0.0; max_frames as usize * self.output_channels];
        let latency = bridge.and_then(|bridge| {
            bridge.activate(
                ctx.sample_rate,
                max_frames,
                self.instance.vst3_wanted_outputs(),
            )
        });
        self.active = latency.is_some();
        self.latency = latency.unwrap_or(0);
        if let Some(bridge) = bridge.filter(|_| self.active) {
            self.param_events.read_ranges(bridge);
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

    /// Fills the wire sections of the block: parameter events, then notes.  The
    /// bridge's adapter merges and sorts them by time, as the Echoes process does
    /// for a plugin it hosts itself.
    fn write_events(&mut self, notes: Option<&MidiEventList>) {
        self.wire_params.events.clear();
        self.param_events
            .fill_into(&self.instance, &mut self.wire_params);
        self.wire_notes.clear();
        if let Some(notes) = notes {
            push_wire_notes(notes, &mut self.wire_notes);
        }
    }

    fn replay_edits(&mut self, edit_count: usize) {
        for edit in &self.edits[..edit_count] {
            self.param_events.observe_edit(edit);
        }
    }

    /// Runs one block through the bridge.  `false` when the block was not sent
    /// (plugin inactive, block too large, too many events, port busy): `output`
    /// is then untouched and the caller decides what to play.
    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
        notes: Option<&MidiEventList>,
    ) -> bool {
        let frames = transport.block_size;
        let input_samples = frames * self.input_channels;
        let output_samples = frames * self.output_channels;
        if !self.active || input_samples > self.input.len() || output_samples > self.output.len() {
            return false;
        }
        self.param_events.begin_block(transport);
        self.write_events(notes);
        let wire_transport = wire_transport(transport);
        for (index, planar) in self
            .input
            .chunks_exact_mut(frames)
            .take(self.input_channels)
            .enumerate()
        {
            let (port, channel) = (index / PORT_CHANNELS, index % PORT_CHANNELS);
            match inputs.get(port) {
                Some(source) => planar.copy_from_slice(&source.channel(channel)[..frames]),
                None => planar.fill(0.0),
            }
        }

        let budget = self.wait_budget(frames);
        let Some(bridge) = self.instance.bridge() else {
            return false;
        };
        let request = BlockRequest {
            frames,
            input: (!inputs.is_empty()).then_some(&self.input[..input_samples]),
            input_channels: self.input_channels,
            output_channels: self.output_channels,
            transport: Some(&wire_transport),
            notes: &self.wire_notes,
            params: &self.wire_params.events,
            sequence: self.sequence,
            steady_time: self.steady_time,
            reset: self.pending_reset,
        };
        let mut reply = BlockReply {
            output: &mut self.output,
            edits: &mut self.edits,
            midi: &mut self.midi_out,
        };
        let outcome = bridge.audio().process_block(&request, &mut reply, budget);
        let condemned = outcome == BlockOutcome::TimedOut && bridge.is_dead();
        self.sequence += 1;
        self.steady_time += frames as i64;

        match outcome {
            BlockOutcome::Skipped => return false,
            BlockOutcome::Processed {
                flags, edit_count, ..
            } => {
                self.pending_reset = false;
                self.replay_edits(edit_count);
                if flags & BLOCK_PLUGIN_EDITED != 0 {
                    mark_plugin_state_edited();
                }
                if flags & BLOCK_RESTART_REQUESTED != 0 {
                    if let Some(bridge) = self.instance.bridge() {
                        bridge.request_restart();
                    }
                }
                if flags & BLOCK_NOTE_NAMES_CHANGED != 0 {
                    mark_plugin_note_names_changed();
                }
                if flags & BLOCK_PARAMS_RESCAN_MASK != 0 {
                    mark_plugin_params_rescan(rescan_flags(
                        (flags & BLOCK_PARAMS_RESCAN_MASK) >> BLOCK_PARAMS_RESCAN_SHIFT,
                    ));
                }
            }
            BlockOutcome::TimedOut if condemned => {
                self.instance.set_lifecycle(LifecycleState::Error);
                note_crash();
            }
            BlockOutcome::TimedOut | BlockOutcome::Dead => {}
        }
        let carried_ports = self.output_ports();
        for (port, buffer) in outputs.iter_mut().enumerate().take(carried_ports) {
            for channel in 0..PORT_CHANNELS {
                let at = (port * PORT_CHANNELS + channel) * frames;
                buffer.channel_mut(channel)[..frames]
                    .copy_from_slice(&self.output[at..at + frames]);
            }
        }
        true
    }

    /// Input ports a block carries, the main one first.
    fn input_ports(&self) -> usize {
        self.input_channels / PORT_CHANNELS
    }

    /// Output ports a block carries, the main one first.
    fn output_ports(&self) -> usize {
        self.output_channels / PORT_CHANNELS
    }

    fn set_param_controls(&mut self, controls: Vec<PluginParamControl>) {
        self.param_events.set_controls(controls, &self.instance);
    }

    fn param_controls(&mut self) -> &mut PluginParamControls {
        self.param_events.controls_mut()
    }
}

fn pass_through(inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer]) {
    if let (Some(input), Some(output)) = (inputs.first(), outputs.first_mut()) {
        output.copy_from(input);
    }
}

/// A sandboxed effect: a block that cannot be sent passes the input through.
pub struct BridgedNode {
    round_trip: BridgeRoundTrip,
}

impl BridgedNode {
    /// `instance` must be sandboxed (`PluginInstance::new_bridged`).
    pub fn new(instance: Arc<PluginInstance>) -> Self {
        BridgedNode {
            round_trip: BridgeRoundTrip::new(instance),
        }
    }
}

impl AudioNode for BridgedNode {
    fn label(&self) -> &str {
        "BridgedPlugin"
    }

    /// Final once `prepare` has run: the main input and the sidechains the bridge's
    /// table carries.
    fn num_input_ports(&self) -> usize {
        self.round_trip.input_ports().max(1)
    }

    /// Final once `prepare` has run: the ports the bridge's table carries.
    fn num_output_ports(&self) -> usize {
        self.round_trip.output_ports()
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.round_trip.prepare(ctx);
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        if outputs.is_empty() {
            return;
        }
        if !self.round_trip.process(inputs, outputs, transport, None) {
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
        self.round_trip
            .param_controls()
            .modulate_at(param_id, sample_offset, value);
    }

    fn apply_automation(&mut self, param_id: u32, value: f32) {
        self.round_trip.param_controls().automate(param_id, value);
    }

    fn apply_automation_at(&mut self, param_id: u32, sample_offset: u32, value: f32) {
        self.round_trip
            .param_controls()
            .automate_at(param_id, sample_offset, value);
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
    fn num_output_ports(&self) -> usize {
        self.round_trip.output_ports()
    }

    /// The plugin's own latency, as in the Echoes process (ADR-013).
    fn latency_samples(&self) -> u32 {
        self.round_trip.latency
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.round_trip.prepare(ctx);
    }

    fn process_midi(
        &mut self,
        events: &MidiEventList,
        outputs: &mut [AudioBuffer],
        transport: &TransportInfo,
    ) {
        if !outputs.is_empty() {
            self.round_trip
                .process(&[], outputs, transport, Some(events));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::midi::MidiEvent;
    use crate::audio_graph::node::TimeSignature;
    use crate::plugin_formats::clap::ffi::{transport_event, TransportValues};
    use crate::plugin_formats::clap::transport::clap_transport_event;

    fn transport(seconds: f64, time_signature: TimeSignature, is_playing: bool) -> TransportInfo {
        TransportInfo {
            is_playing,
            position_samples: (seconds * 48_000.0) as u64,
            position_seconds: seconds,
            bpm: 117.5,
            time_signature,
            sample_rate: 48_000.0,
            block_size: 256,
        }
    }

    /// What the bridge's CLAP adapter does with the wire transport.
    fn adapter_event(wire: &WireTransport) -> crate::plugin_formats::clap::ffi::ClapEventTransport {
        transport_event(&TransportValues {
            playing: wire.flags & TRANSPORT_PLAYING != 0,
            position_beats: wire.position_beats,
            position_seconds: wire.position_seconds,
            tempo: wire.tempo,
            bar_start_beats: wire.bar_start_beats,
            bar_number: wire.bar_number,
            time_signature_numerator: wire.time_signature_numerator,
            time_signature_denominator: wire.time_signature_denominator,
        })
    }

    #[test]
    fn the_wire_transport_gives_the_clap_adapter_the_in_process_event() {
        for info in [
            transport(4.0, TimeSignature::new(3, 4), true),
            transport(1.5, TimeSignature::new(6, 8), false),
            transport(0.0, TimeSignature::new(4, 4), false),
        ] {
            let wire = wire_transport(&info);
            let direct = clap_transport_event(&info);
            let relayed = adapter_event(&wire);
            assert_eq!(
                (
                    relayed.flags,
                    relayed.song_pos_beats,
                    relayed.song_pos_seconds
                ),
                (direct.flags, direct.song_pos_beats, direct.song_pos_seconds)
            );
            assert_eq!(
                (relayed.tempo, relayed.bar_start, relayed.bar_number),
                (direct.tempo, direct.bar_start, direct.bar_number)
            );
            assert_eq!(
                (relayed.tsig_num, relayed.tsig_denom),
                (direct.tsig_num, direct.tsig_denom)
            );
        }
    }

    #[test]
    fn notes_and_controls_keep_their_order_and_offsets() {
        let mut list = MidiEventList::new();
        list.push(MidiEvent {
            offset: 3,
            kind: MidiEventKind::NoteOn {
                pitch: 60,
                velocity: 100,
            },
        });
        list.push(MidiEvent {
            offset: 3,
            kind: MidiEventKind::Control {
                message: [0xE0, 1, 2],
            },
        });
        list.push(MidiEvent {
            offset: 9,
            kind: MidiEventKind::NoteOff { pitch: 60 },
        });
        let mut notes = Vec::with_capacity(8);
        push_wire_notes(&list, &mut notes);
        assert_eq!(
            notes,
            [
                WireNote::new(3, NOTE_ON, [60, 100, 0]),
                WireNote::new(3, MIDI, [0xE0, 1, 2]),
                WireNote::new(9, NOTE_OFF, [60, 0, 0]),
            ]
        );
    }

    #[test]
    fn a_note_expression_travels_with_its_key_kind_and_exact_value() {
        let mut list = MidiEventList::new();
        for (offset, kind, value) in [
            (0, NoteExpressionKind::Pressure, 0.75),
            (2, NoteExpressionKind::Timbre, 0.1),
            (4, NoteExpressionKind::Pitch, -3.5),
        ] {
            list.push(MidiEvent {
                offset,
                kind: MidiEventKind::NoteExpression {
                    pitch: 64,
                    kind,
                    value,
                },
            });
        }
        let mut notes = Vec::with_capacity(8);
        push_wire_notes(&list, &mut notes);
        assert_eq!(
            notes,
            [
                WireNote::expression(0, 64, EXPRESSION_PRESSURE, 0.75),
                WireNote::expression(2, 64, EXPRESSION_TIMBRE, 0.1),
                WireNote::expression(4, 64, EXPRESSION_PITCH, -3.5),
            ]
        );
    }

    #[test]
    fn the_notes_vector_never_grows() {
        let mut list = MidiEventList::new();
        for offset in 0..4 {
            list.push(MidiEvent {
                offset,
                kind: MidiEventKind::NoteOff { pitch: 60 },
            });
        }
        let mut notes = Vec::with_capacity(3);
        push_wire_notes(&list, &mut notes);
        assert_eq!((notes.len(), notes.capacity()), (3, 3));
    }

    #[test]
    fn the_parameter_sink_drops_what_does_not_fit() {
        let mut sink = WireParamSink::new();
        for index in 0..MAX_PARAM_EVENTS as u32 + 10 {
            ParamEventSink::push_value(&mut sink, index, 1, 0.5);
        }
        sink.push_modulation(0, 2, 0.1);
        assert_eq!(sink.events.len(), MAX_PARAM_EVENTS);
        assert_eq!(sink.events[0], WireParam::new(PARAM_VALUE, 0, 1, 0.5));
        assert_eq!(sink.events.capacity(), MAX_PARAM_EVENTS, "never grows");
    }

    #[test]
    fn a_vst3_value_travels_as_the_same_record_whatever_its_unit() {
        let mut sink = WireParamSink::new();
        Vst3ParamSink::push_value(&mut sink, 7, 3, 0.25);
        assert_eq!(sink.events, [WireParam::new(PARAM_VALUE, 7, 3, 0.25)]);
    }

    #[test]
    fn rescan_bits_map_back_to_the_clap_flags_the_app_reads() {
        assert_eq!(rescan_flags(0), 0);
        assert_eq!(
            rescan_flags(PARAMS_TEXT | PARAMS_INFO),
            CLAP_PARAM_RESCAN_TEXT | CLAP_PARAM_RESCAN_INFO
        );
        assert_eq!(
            rescan_flags(PARAMS_LIST | PARAMS_VALUES),
            CLAP_PARAM_RESCAN_ALL | CLAP_PARAM_RESCAN_VALUES
        );
    }
}
