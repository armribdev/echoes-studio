//! The VST3 adapter: the effect hosted by this bridge process, seen through the
//! format-neutral protocol (ADR-033, step 5).
//!
//! One component and its processor, on one stereo main bus each way.  The
//! auxiliary buses are declared in the handshake but stay closed until the port
//! table carries more than one active port (step 8).  There is no edit
//! controller yet: parameter lists, the editor and presets come with steps 6, 7
//! and 9, and answer an explicit error until then.
//!
//! ## Threads
//!
//! The main thread loads, activates, saves and drops the plugin; the audio
//! thread only calls `process`.  VST3 forbids those calls from overlapping, so
//! both go through one gate of atomics (ADR-010): the audio thread never waits —
//! it skips its block, passing the input through — while the main thread waits
//! a bounded time for a block in flight.  The activation, and the buffers it
//! sized, belong to whoever holds the gate.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use audio_buffer::AudioBuffer;
use plugin_bridge::control::{Op, PayloadReader};
use plugin_bridge::protocol::{BLOCK_PASSED_THROUGH, PARAM_VALUE, TRANSPORT_PLAYING};
use plugin_bridge::records::{
    AudioPortRecord, Handshake, NoteNameRecord, ParamRecord, ParamUnit, PortDirection, PortRole,
    SpeakerLayout,
};
use plugin_bridge::session::{BlockContext, BridgeProcessor, ReplyWriter};
use vst3_ffi::{
    instantiate_processor, read_component_state, write_component_state, HostParameterChanges,
    IAudioProcessor, IComponent, ProcessData, ProcessSetup, Vst3Library, K_AUDIO, K_EVENT, K_INPUT,
    K_RESULT_OK,
};
use vst3_host::buses::{
    open_output_buses, propose_arrangements, read_aux_inputs, read_output_buses, Vst3InputBuses,
    Vst3OutputBuses,
};
use vst3_host::host_context::host_application;
use vst3_host::transport::{process_context, TransportState};

use crate::HostedPlugin;

/// How long the main thread waits for the audio thread to finish a block.
const GATE_BUDGET: Duration = Duration::from_secs(2);

const GATE_IDLE: u8 = 0;
const GATE_AUDIO: u8 = 1;
const GATE_MAIN: u8 = 2;

struct GateGuard<'a>(&'a AtomicU8);

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        self.0.store(GATE_IDLE, Ordering::Release);
    }
}

/// What one activation sized; replaced as a whole by the next one.
struct Activation {
    max_frames: u32,
    inputs: Vst3InputBuses,
    outputs: Vst3OutputBuses,
    input: AudioBuffer,
    output: Vec<AudioBuffer>,
    param_changes: Box<HostParameterChanges>,
}

/// `(sample rate, max frames, latency)` of the current activation.
type Configuration = (f64, u32, u32);

pub struct Vst3Adapter {
    component: *mut IComponent,
    processor: *mut IAudioProcessor,
    /// Channels of the main bus each way, as the engine's blocks carry them.
    channels: usize,
    gate: AtomicU8,
    /// Read and written only while holding `gate`.
    activation: UnsafeCell<Option<Activation>>,
    /// Main thread.
    configuration: Mutex<Option<Configuration>>,
    /// Must outlive the plugin objects: dropping it runs the module's `ExitDll`.
    _library: Arc<Vst3Library>,
}

// SAFETY: the plugin is reached from the main and audio threads only through
// the gate above, which keeps `process` and every other call apart.
unsafe impl Send for Vst3Adapter {}
unsafe impl Sync for Vst3Adapter {}

impl Vst3Adapter {
    /// Main thread.  Loads the module and creates the component and its
    /// processor for class id `cid` (32 hexadecimal digits).
    pub fn load(dll: &Path, cid: &str, channels: usize) -> Result<Self, String> {
        let cid = decode_cid(cid).ok_or_else(|| format!("invalid VST3 class id '{cid}'"))?;
        let (library, factory) = Vst3Library::load(dll)?;
        // SAFETY: live factory, called off the audio thread.
        let created = unsafe { instantiate_processor(factory, &cid, host_application()) };
        // SAFETY: the factory was returned with a reference this call owns.
        unsafe { ((*(*factory).vtbl).release)(factory as *mut _) };
        let (component, processor) = created?;
        Ok(Vst3Adapter {
            component,
            processor,
            channels,
            gate: AtomicU8::new(GATE_IDLE),
            activation: UnsafeCell::new(None),
            configuration: Mutex::new(None),
            _library: library,
        })
    }

    fn enter_audio(&self) -> Option<GateGuard<'_>> {
        self.gate
            .compare_exchange(GATE_IDLE, GATE_AUDIO, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| GateGuard(&self.gate))
    }

    fn enter_main(&self) -> Result<GateGuard<'_>, String> {
        let deadline = Instant::now() + GATE_BUDGET;
        loop {
            if self
                .gate
                .compare_exchange(GATE_IDLE, GATE_MAIN, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(GateGuard(&self.gate));
            }
            if Instant::now() >= deadline {
                return Err("the plugin is busy processing".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Main thread.  Activates at `(sample_rate, max_frames)` — a no-op when
    /// already active there — and returns the plugin's latency.
    pub fn activate(&self, sample_rate: f64, max_frames: u32) -> Result<u32, String> {
        let mut configuration = self.configuration.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((rate, frames, latency)) = *configuration {
            if rate == sample_rate && frames == max_frames {
                return Ok(latency);
            }
        }
        *configuration = None;
        let _gate = self.enter_main()?;
        // SAFETY: the main-thread gate is held, so no block is in flight.
        let latency = unsafe {
            self.stop();
            let activation = self.start(sample_rate, max_frames)?;
            *self.activation.get() = Some(activation);
            ((*(*self.processor).vtbl).get_latency_samples)(self.processor)
        };
        *configuration = Some((sample_rate, max_frames, latency));
        Ok(latency)
    }

    /// Stops processing and deactivates, when activated.
    ///
    /// # Safety
    /// Main thread, gate held.
    unsafe fn stop(&self) {
        if (*self.activation.get()).take().is_some() {
            ((*(*self.processor).vtbl).set_processing)(self.processor, 0);
            ((*(*self.component).vtbl).set_active)(self.component, 0);
        }
    }

    /// The VST3 order: bus setup and `setupProcessing` while inactive, then
    /// `setActive`, then `setProcessing`.
    ///
    /// # Safety
    /// Main thread, gate held, plugin not active.
    unsafe fn start(&self, sample_rate: f64, max_frames: u32) -> Result<Activation, String> {
        let (component, processor) = (self.component, self.processor);
        let has_audio_input = propose_arrangements(component, processor);
        let has_event_input = ((*(*component).vtbl).get_bus_count)(component, K_EVENT, K_INPUT) > 0;
        if has_audio_input {
            ((*(*component).vtbl).activate_bus)(component, K_AUDIO, K_INPUT, 0, 1);
        }
        let mut output_buses = read_output_buses(component);
        open_output_buses(component, &mut output_buses, 0, 0);
        if has_event_input {
            ((*(*component).vtbl).activate_bus)(component, K_EVENT, K_INPUT, 0, 1);
        }
        let setup = ProcessSetup {
            process_mode: 0,
            symbolic_sample_size: 0,
            max_samples_per_block: max_frames as i32,
            sample_rate,
        };
        ((*(*processor).vtbl).setup_processing)(processor, &setup);
        if ((*(*component).vtbl).set_active)(component, 1) != K_RESULT_OK {
            return Err("the plugin refused setActive".into());
        }
        ((*(*processor).vtbl).set_processing)(processor, 1);

        let channels = self.channels;
        let mut inputs = Vst3InputBuses::new(&[]);
        inputs.prepare(channels, max_frames as usize);
        let mut outputs = Vst3OutputBuses::new(&output_buses);
        outputs.prepare(channels, max_frames as usize);
        Ok(Activation {
            max_frames,
            inputs,
            outputs,
            input: AudioBuffer::new(channels, max_frames as usize),
            output: vec![AudioBuffer::new(channels, max_frames as usize)],
            param_changes: HostParameterChanges::boxed(),
        })
    }

    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        let _gate = self.enter_main()?;
        // SAFETY: main thread, gate held.
        unsafe { read_component_state(self.component) }
            .ok_or_else(|| "the plugin has no state".into())
    }

    pub fn load_state(&self, bytes: &[u8]) -> Result<(), String> {
        let _gate = self.enter_main()?;
        // SAFETY: main thread, gate held.
        if unsafe { write_component_state(self.component, bytes) } {
            Ok(())
        } else {
            Err("the plugin rejected the state".into())
        }
    }

    /// The audio ports of the plugin; the main bus of each direction is the
    /// active one, as the block carries one input and one output.
    pub fn handshake(&self) -> Vec<u8> {
        let channels = self.channels as u32;
        let mut ports = Vec::new();
        // SAFETY: main thread, plugin initialised, no process in flight before
        // the first activation.
        let (input_buses, output_buses) = unsafe {
            let inputs =
                ((*(*self.component).vtbl).get_bus_count)(self.component, K_AUDIO, K_INPUT);
            (
                (inputs > 0).then(|| read_aux_inputs(self.component)),
                read_output_buses(self.component),
            )
        };
        if let Some(auxiliary) = input_buses {
            ports.push(main_port(PortDirection::Input, channels));
            ports.extend(
                auxiliary
                    .iter()
                    .map(|bus| auxiliary_port(PortDirection::Input, bus.channel_count)),
            );
        }
        for (index, bus) in output_buses.iter().enumerate() {
            ports.push(if index == 0 {
                main_port(PortDirection::Output, channels)
            } else {
                auxiliary_port(PortDirection::Output, bus.channel_count)
            });
        }
        Handshake {
            param_unit: ParamUnit::Normalised,
            ports,
        }
        .encode()
    }
}

fn main_port(direction: PortDirection, channels: u32) -> AudioPortRecord {
    AudioPortRecord {
        direction,
        role: PortRole::Main,
        channels,
        layout: layout_of(channels),
        active: true,
    }
}

fn auxiliary_port(direction: PortDirection, channels: u32) -> AudioPortRecord {
    AudioPortRecord {
        direction,
        role: PortRole::Auxiliary,
        channels,
        layout: layout_of(channels),
        active: false,
    }
}

fn layout_of(channels: u32) -> SpeakerLayout {
    match channels {
        1 => SpeakerLayout::Mono,
        2 => SpeakerLayout::Stereo,
        _ => SpeakerLayout::Other,
    }
}

fn decode_cid(hex: &str) -> Option<[u8; 16]> {
    if hex.len() != 32 {
        return None;
    }
    let mut cid = [0u8; 16];
    for (index, byte) in cid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(cid)
}

impl Drop for Vst3Adapter {
    // Runs on the main thread after the audio thread has exited.
    fn drop(&mut self) {
        // SAFETY: VST3 teardown order; no other thread uses the plugin any more.
        unsafe {
            self.stop();
            ((*(*self.component).vtbl).terminate)(self.component);
            // IAudioProcessor and IComponent may be the same COM object; release each.
            ((*(*self.processor).vtbl).release)(self.processor as *mut _);
            ((*(*self.component).vtbl).release)(self.component as *mut _);
        }
    }
}

impl HostedPlugin for Vst3Adapter {
    fn handle(&self, op: Op, payload: &[u8]) -> Result<Vec<u8>, String> {
        let invalid = |e: std::io::Error| e.to_string();
        match op {
            Op::Activate => {
                let mut reader = PayloadReader::new(payload);
                let sample_rate = reader.f64().map_err(invalid)?;
                let max_frames = reader.u32().map_err(invalid)?;
                self.activate(sample_rate, max_frames)
                    .map(|latency| latency.to_le_bytes().to_vec())
            }
            Op::SaveState => self.save_state(),
            Op::LoadState => self.load_state(payload).map(|()| Vec::new()),
            Op::Handshake => Ok(self.handshake()),
            Op::Params => Ok(ParamRecord::encode_list(&[])),
            Op::NoteNames => Ok(NoteNameRecord::encode_list(&[])),
            Op::ParamText | Op::FlushValues | Op::TrackInfo => Ok(Vec::new()),
            Op::ParamValue => Err("a sandboxed VST3 plugin has no parameters yet".into()),
            Op::PresetLoad => {
                Err("presets of a sandboxed VST3 plugin are not available yet".into())
            }
            Op::OpenEditor => {
                Err("the editor of a sandboxed VST3 plugin is not available yet".into())
            }
        }
    }
}

/// Drives the plugin's `process` from the bridge's audio loop.
pub struct Vst3BlockProcessor {
    plugin: Arc<Vst3Adapter>,
}

// SAFETY: moved once into the audio thread, which is its only user.
unsafe impl Send for Vst3BlockProcessor {}

impl Vst3BlockProcessor {
    pub fn new(plugin: Arc<Vst3Adapter>) -> Self {
        Vst3BlockProcessor { plugin }
    }
}

fn pass_through(input: &[f32], output: &mut [f32]) -> u32 {
    output.copy_from_slice(input);
    BLOCK_PASSED_THROUGH
}

fn transport_state(block: &BlockContext) -> Option<TransportState> {
    block.transport.map(|transport| TransportState {
        playing: transport.flags & TRANSPORT_PLAYING != 0,
        sample_rate: transport.sample_rate,
        position_samples: transport.position_samples,
        position_beats: transport.position_beats,
        bar_start_beats: transport.bar_start_beats,
        tempo: transport.tempo,
        time_signature_numerator: i32::from(transport.time_signature_numerator),
        time_signature_denominator: i32::from(transport.time_signature_denominator),
    })
}

impl BridgeProcessor for Vst3BlockProcessor {
    fn process(
        &mut self,
        block: &BlockContext,
        input: &[f32],
        output: &mut [f32],
        _reply: &mut ReplyWriter,
    ) -> u32 {
        let plugin = &*self.plugin;
        let Some(_gate) = plugin.enter_audio() else {
            return pass_through(input, output);
        };
        // SAFETY: the audio gate is held, so the main thread leaves the activation alone.
        let Some(activation) = (unsafe { &mut *plugin.activation.get() }) else {
            return pass_through(input, output);
        };
        let frames = block.frames;
        if frames > activation.max_frames as usize || block.channels > activation.input.num_channels
        {
            return pass_through(input, output);
        }

        if block.reset {
            // SAFETY: audio gate held; setProcessing(false) then (true) resets the plugin.
            unsafe {
                ((*(*plugin.processor).vtbl).set_processing)(plugin.processor, 0);
                ((*(*plugin.processor).vtbl).set_processing)(plugin.processor, 1);
            }
        }

        for channel in 0..block.channels {
            activation.input.channel_mut(channel)[..frames]
                .copy_from_slice(&input[channel * frames..(channel + 1) * frames]);
        }

        activation.param_changes.reset();
        for param in block.params.iter() {
            if param.kind == PARAM_VALUE {
                activation
                    .param_changes
                    .push(param.native_id, param.offset as i32, param.value);
            }
        }
        let param_changes = if activation.param_changes.count > 0 {
            activation.param_changes.as_mut() as *mut HostParameterChanges as *mut c_void
        } else {
            std::ptr::null_mut()
        };

        let sources: [&AudioBuffer; 1] = [&activation.input];
        let (input_buses, input_bus_count) = activation
            .inputs
            .bind(if block.has_input { &sources } else { &[] }, frames);
        let (output_buses, output_bus_count) = activation.outputs.bind(&mut activation.output);

        let mut context = transport_state(block).map(|state| process_context(&state));
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: frames as i32,
            num_inputs: input_bus_count,
            num_outputs: output_bus_count,
            inputs: input_buses,
            outputs: output_buses,
            input_param_changes: param_changes,
            output_param_changes: std::ptr::null_mut(),
            input_events: std::ptr::null_mut(),
            output_events: std::ptr::null_mut(),
            process_context: context
                .as_mut()
                .map_or(std::ptr::null_mut(), |c| c as *mut _ as *mut c_void),
        };
        // SAFETY: audio gate held; every buffer outlives the call.
        let result = unsafe { ((*(*plugin.processor).vtbl).process)(plugin.processor, &mut data) };
        if result != K_RESULT_OK {
            return pass_through(input, output);
        }
        for channel in 0..block.channels {
            output[channel * frames..(channel + 1) * frames]
                .copy_from_slice(&activation.output[0].channel(channel)[..frames]);
        }
        0
    }
}
