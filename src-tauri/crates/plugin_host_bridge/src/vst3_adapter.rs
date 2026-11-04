//! The VST3 adapter: the effect hosted by this bridge process, seen through the
//! format-neutral protocol (ADR-033, steps 5 and 6).
//!
//! One component, its processor and its edit controller, on one stereo main bus
//! each way.  The auxiliary buses are declared in the handshake but stay closed
//! until the port table carries more than one active port (step 8).  Presets come
//! with step 9 and answer an explicit error until then.
//!
//! ## Parameters
//!
//! Values are normalised end to end and the bridge converts and computes nothing:
//! the engine sends the effective value of each point (automation, modulation
//! around its base) and keeps the bases.  What the plugin does by itself comes
//! back in the block's reply — window edits as parameter records, gestures as
//! begin / end records, and the unsaved, restart and parameter-list signals of
//! its component handler as reply flags.  What the plugin's processor writes to
//! `outputParameterChanges` (a program, a meter) is kept for its controller only,
//! as in the Echoes process.
//!
//! ## Threads
//!
//! The main thread loads, activates, saves and drops the plugin and answers the
//! control requests; the audio thread only calls `process`.  VST3 forbids those
//! calls from overlapping, so both go through one gate of atomics (ADR-010): the
//! audio thread never waits — it skips its block, passing the input through —
//! while the main thread waits a bounded time for a block in flight.  The
//! activation, and the buffers it sized, belong to whoever holds the gate.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use audio_buffer::AudioBuffer;
use plugin_bridge::control::{Op, PayloadReader};
use plugin_bridge::protocol::{
    WireParam, BLOCK_PARAMS_RESCAN_MASK, BLOCK_PARAMS_RESCAN_SHIFT, BLOCK_PASSED_THROUGH,
    BLOCK_PLUGIN_EDITED, BLOCK_RESTART_REQUESTED, PARAMS_INFO, PARAMS_TEXT, PARAM_GESTURE_BEGIN,
    PARAM_GESTURE_END, PARAM_VALUE, TRANSPORT_PLAYING,
};
use plugin_bridge::records::{
    AudioPortRecord, Handshake, NoteNameRecord, ParamRecord, ParamUnit, PortDirection, PortRole,
    SpeakerLayout, PARAM_AUTOMATABLE, PARAM_HIDDEN,
};
use plugin_bridge::session::{BlockContext, BridgeProcessor, ReplyWriter};
use vst3_ffi::{
    create_controller, instantiate_processor, read_component_state, release_controller,
    set_controller_component_state, sync_controller_to_state, write_component_state,
    HostParameterChanges, IAudioProcessor, IComponent, ProcessData, ProcessSetup, Vst3Controller,
    Vst3Library, K_AUDIO, K_EVENT, K_INPUT, K_RESULT_OK,
};
use vst3_host::buses::{
    open_output_buses, propose_arrangements, read_aux_inputs, read_output_buses, Vst3InputBuses,
    Vst3OutputBuses,
};
use vst3_host::component_handler::{ComponentHandler, EditState, HostSignals, MAX_GESTURES};
use vst3_host::editor_view::{EditorView, ResizeWindow};
use vst3_host::host_context::host_application;
use vst3_host::params::{
    read_vst3_param_text, read_vst3_param_value, read_vst3_params, write_vst3_param_value,
};
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

/// What the plugin's component handler reported from its window thread, until the
/// audio thread puts it in the reply of the next block.
#[derive(Default)]
struct BridgeSignals {
    edited: AtomicBool,
    restart: AtomicBool,
    params: AtomicU32,
}

impl HostSignals for BridgeSignals {
    fn state_edited(&self) {
        self.edited.store(true, Ordering::Release);
    }

    fn restart_requested(&self) {
        self.restart.store(true, Ordering::Release);
    }

    fn params_changed(&self, titles_too: bool) {
        let titles = if titles_too { PARAMS_INFO } else { 0 };
        self.params.fetch_or(PARAMS_TEXT | titles, Ordering::AcqRel);
    }
}

impl BridgeSignals {
    /// Audio thread: the `BLOCK_*` reply flags for what was reported since the last
    /// block.
    fn take_reply_flags(&self) -> u32 {
        let mut flags = 0;
        if self.edited.swap(false, Ordering::AcqRel) {
            flags |= BLOCK_PLUGIN_EDITED;
        }
        if self.restart.swap(false, Ordering::AcqRel) {
            flags |= BLOCK_RESTART_REQUESTED;
        }
        let params = self.params.swap(0, Ordering::AcqRel);
        flags | ((params << BLOCK_PARAMS_RESCAN_SHIFT) & BLOCK_PARAMS_RESCAN_MASK)
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
    /// Where the plugin writes the changes it makes itself, for its controller.
    output_changes: Box<HostParameterChanges>,
}

/// `(sample rate, max frames, latency)` of the current activation.
type Configuration = (f64, u32, u32);

pub struct Vst3Adapter {
    component: *mut IComponent,
    processor: *mut IAudioProcessor,
    /// `None` when the plugin has no edit controller: it still plays, without a
    /// parameter list.
    controller: Option<Vst3Controller>,
    /// The controller's `IComponentHandler`; outlives the controller.
    handler: Box<ComponentHandler>,
    signals: Arc<BridgeSignals>,
    /// Channels of the main bus each way, as the engine's blocks carry them.
    channels: usize,
    gate: AtomicU8,
    /// Read and written only while holding `gate`.
    activation: UnsafeCell<Option<Activation>>,
    /// Main thread.
    configuration: Mutex<Option<Configuration>>,
    /// Parameters the engine modulates by value, at the base the user set: put back
    /// in the plugin just before its state is read.  Main thread.
    bases: Mutex<Vec<(u32, f64)>>,
    /// The shared handle this adapter lives in, for the editor, which keeps it open.
    me: Weak<Vst3Adapter>,
    /// Must outlive the plugin objects: dropping it runs the module's `ExitDll`.
    _library: Arc<Vst3Library>,
}

// SAFETY: the plugin is reached from the main and audio threads only through
// the gate above, which keeps `process` and every other call apart.
unsafe impl Send for Vst3Adapter {}
unsafe impl Sync for Vst3Adapter {}

impl Vst3Adapter {
    /// Main thread.  Loads the module and creates the component, its processor and
    /// its edit controller for class id `cid` (32 hexadecimal digits).
    pub fn load(dll: &Path, cid: &str, channels: usize) -> Result<Arc<Self>, String> {
        let cid = decode_cid(cid).ok_or_else(|| format!("invalid VST3 class id '{cid}'"))?;
        let (library, factory) = Vst3Library::load(dll)?;
        // SAFETY: live factory, called off the audio thread.
        let created = unsafe { instantiate_processor(factory, &cid, host_application()) };
        // SAFETY: the factory was returned with a reference this call owns.
        unsafe { ((*(*factory).vtbl).release)(factory as *mut _) };
        let (component, processor) = created?;

        let signals = Arc::new(BridgeSignals::default());
        let handler = ComponentHandler::boxed(Arc::clone(&signals) as Arc<dyn HostSignals>);
        // SAFETY: the component was initialised by `instantiate_processor`; main thread.
        let controller = match unsafe {
            create_controller(
                &library,
                component,
                host_application(),
                handler.as_funknown(),
            )
        } {
            Ok(controller) => Some(controller),
            Err(error) => {
                eprintln!("[plugin_host_bridge] the plugin has no edit controller: {error}");
                None
            }
        };
        Ok(Arc::new_cyclic(|me| Vst3Adapter {
            component,
            processor,
            controller,
            handler,
            signals,
            channels,
            gate: AtomicU8::new(GATE_IDLE),
            activation: UnsafeCell::new(None),
            configuration: Mutex::new(None),
            bases: Mutex::new(Vec::new()),
            me: Weak::clone(me),
            _library: library,
        }))
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

    fn edit_state(&self) -> &EditState {
        self.handler.state()
    }

    fn controller_ptr(&self) -> Result<*mut vst3_ffi::IEditController, String> {
        self.controller
            .as_ref()
            .map(Vst3Controller::as_ptr)
            .ok_or_else(|| "the plugin has no edit controller".into())
    }

    /// Main thread.  Activates at `(sample_rate, max_frames)` and returns the
    /// plugin's latency.  A no-op when already active there, unless the plugin
    /// asked to be reactivated (`restartComponent`), which reads the latency again.
    pub fn activate(&self, sample_rate: f64, max_frames: u32) -> Result<u32, String> {
        let mut configuration = self.configuration.lock().unwrap_or_else(|e| e.into_inner());
        let restart = self.edit_state().take_reactivation_request();
        if let Some((rate, frames, latency)) = *configuration {
            if !restart && rate == sample_rate && frames == max_frames {
                return Ok(latency);
            }
        }
        *configuration = None;
        let _gate = match self.enter_main() {
            Ok(gate) => gate,
            Err(error) => {
                if restart {
                    self.edit_state().request_reactivation();
                }
                return Err(error);
            }
        };
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
            output_changes: HostParameterChanges::boxed(),
        })
    }

    /// The processor state, with the parameters modulated by value first put back
    /// on the base the user set (ADR-021), in one go: no block lands in between.
    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        let _gate = self.enter_main()?;
        let bases = std::mem::take(&mut *self.bases.lock().unwrap_or_else(|e| e.into_inner()));
        // SAFETY: main thread, gate held.
        unsafe {
            self.flush_bases(&bases);
            read_component_state(self.component)
        }
        .ok_or_else(|| "the plugin has no state".into())
    }

    /// Loads the processor state, then hands the controller what its processor now
    /// holds, as the Echoes process does for a separate controller.
    pub fn load_state(&self, bytes: &[u8]) -> Result<(), String> {
        let _gate = self.enter_main()?;
        self.bases.lock().unwrap_or_else(|e| e.into_inner()).clear();
        // SAFETY: main thread, gate held.
        let loaded = unsafe { write_component_state(self.component, bytes) };
        if !loaded {
            return Err("the plugin rejected the state".into());
        }
        if let Some(controller) = &self.controller {
            // SAFETY: main thread, gate held.
            unsafe { sync_controller_to_state(controller, bytes) };
        }
        Ok(())
    }

    /// Puts the parameters modulated by value on their base now, and again just
    /// before the next state save.
    pub fn flush_values(&self, values: Vec<(u32, f64)>) -> Result<(), String> {
        let _gate = self.enter_main()?;
        // SAFETY: main thread, gate held.
        unsafe { self.flush_bases(&values) };
        *self.bases.lock().unwrap_or_else(|e| e.into_inner()) = values;
        Ok(())
    }

    /// The parameter flush `ProcessData` allows: a zero-sample `process` that only
    /// carries parameter changes.  Nothing happens while the plugin is inactive.
    ///
    /// # Safety
    /// Main thread, gate held.
    unsafe fn flush_bases(&self, bases: &[(u32, f64)]) {
        if bases.is_empty() || (*self.activation.get()).is_none() {
            return;
        }
        let mut changes = HostParameterChanges::boxed();
        for &(native_id, value) in bases {
            changes.push(native_id, 0, value);
        }
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: 0,
            num_inputs: 0,
            num_outputs: 0,
            inputs: std::ptr::null_mut(),
            outputs: std::ptr::null_mut(),
            input_param_changes: changes.as_mut() as *mut HostParameterChanges as *mut c_void,
            output_param_changes: std::ptr::null_mut(),
            input_events: std::ptr::null_mut(),
            output_events: std::ptr::null_mut(),
            process_context: std::ptr::null_mut(),
        };
        ((*(*self.processor).vtbl).process)(self.processor, &mut data);
    }

    /// The controller's parameters, as the Echoes process lists them.
    fn params(&self) -> Result<Vec<u8>, String> {
        let Ok(controller) = self.controller_ptr() else {
            return Ok(ParamRecord::encode_list(&[]));
        };
        let _gate = self.enter_main()?;
        // SAFETY: live controller, main thread, gate held.
        let params = unsafe { read_vst3_params(controller) };
        let records: Vec<ParamRecord> = params
            .into_iter()
            .map(|param| {
                let mut flags = 0;
                if param.automatable {
                    flags |= PARAM_AUTOMATABLE;
                }
                if param.hidden {
                    flags |= PARAM_HIDDEN;
                }
                ParamRecord {
                    native_id: param.native_id,
                    name: param.name,
                    module: param.module,
                    min: param.min,
                    max: param.max,
                    default: param.default,
                    flags,
                }
            })
            .collect();
        Ok(ParamRecord::encode_list(&records))
    }

    fn param_value(&self, native_id: u32) -> Result<Vec<u8>, String> {
        let controller = self.controller_ptr()?;
        let _gate = self.enter_main()?;
        // SAFETY: live controller, main thread, gate held.
        let value = unsafe { read_vst3_param_value(controller, native_id) };
        Ok(value.to_le_bytes().to_vec())
    }

    /// The plugin's text for a normalised value; empty when it has none.
    fn param_text(&self, native_id: u32, normalised: f64) -> Result<Vec<u8>, String> {
        let Ok(controller) = self.controller_ptr() else {
            return Ok(Vec::new());
        };
        let _gate = self.enter_main()?;
        // SAFETY: live controller, main thread, gate held.
        let text = unsafe { read_vst3_param_text(controller, native_id, normalised) };
        Ok(text.unwrap_or_default().into_bytes())
    }

    /// Window thread: hands the controller the values the engine and the plugin gave
    /// the processor since the last call, so the knobs of an open editor follow.
    /// No gate: a controller is called from its UI thread while the processor runs,
    /// as its own window does.
    pub fn mirror_to_controller(&self) {
        let Some(controller) = &self.controller else {
            return;
        };
        self.edit_state().drain_mirrored(|native_id, value| {
            // SAFETY: live controller, on the thread of its window.
            unsafe { write_vst3_param_value(controller.as_ptr(), native_id, value) }
        });
    }

    /// An editor window opens: the processor's values are kept for its controller
    /// (see [`Self::mirror_to_controller`]) until the guard drops, and the
    /// controller first takes the processor's state so the window opens on current
    /// values.
    pub fn watch_params(self: &Arc<Self>) -> Result<ParamMirrorWatch, String> {
        self.edit_state().mirror().watch();
        let watch = ParamMirrorWatch(Arc::clone(self));
        if let Ok(controller) = self.controller_ptr() {
            let _gate = self.enter_main()?;
            // SAFETY: live component and controller, main thread, gate held.
            if let Some(state) = unsafe { read_component_state(self.component) } {
                unsafe { set_controller_component_state(controller, &state) };
            }
        }
        Ok(watch)
    }

    /// Main thread.  The editor view of the controller, made under the gate like any
    /// other controller call: the audio thread skips its block meanwhile.
    pub fn open_view(&self, resize_window: ResizeWindow) -> Result<EditorView, String> {
        let controller = self.controller_ptr()?;
        let _gate = self.enter_main()?;
        // SAFETY: live controller, main thread, gate held.
        unsafe { EditorView::create(controller, resize_window) }
    }

    /// Main thread.  Opens the editor window, or brings the open one to the front, and
    /// returns its handle.
    #[cfg(windows)]
    fn open_editor(&self, title: &str) -> Result<Vec<u8>, String> {
        let adapter = self.me.upgrade().ok_or("the plugin is closing")?;
        crate::editor::open_vst3(&adapter, title).map(|hwnd| hwnd.to_le_bytes().to_vec())
    }

    #[cfg(not(windows))]
    fn open_editor(&self, _title: &str) -> Result<Vec<u8>, String> {
        Err("plugin editors are only available on Windows".into())
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

/// Keeps the processor's values for the controller of an open editor.
pub struct ParamMirrorWatch(Arc<Vst3Adapter>);

impl Drop for ParamMirrorWatch {
    fn drop(&mut self) {
        self.0.edit_state().mirror().unwatch();
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
            if let Some(controller) = self.controller.take() {
                release_controller(controller);
            }
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
        let mut reader = PayloadReader::new(payload);
        match op {
            Op::Activate => {
                let sample_rate = reader.f64().map_err(invalid)?;
                let max_frames = reader.u32().map_err(invalid)?;
                self.activate(sample_rate, max_frames)
                    .map(|latency| latency.to_le_bytes().to_vec())
            }
            Op::SaveState => self.save_state(),
            Op::LoadState => self.load_state(payload).map(|()| Vec::new()),
            Op::Handshake => Ok(self.handshake()),
            Op::Params => self.params(),
            Op::ParamValue => self.param_value(reader.u32().map_err(invalid)?),
            Op::ParamText => {
                let native_id = reader.u32().map_err(invalid)?;
                let value = reader.f64().map_err(invalid)?;
                self.param_text(native_id, value)
            }
            Op::FlushValues => {
                let mut values = Vec::new();
                while !reader.is_empty() {
                    values.push((
                        reader.u32().map_err(invalid)?,
                        reader.f64().map_err(invalid)?,
                    ));
                }
                self.flush_values(values).map(|()| Vec::new())
            }
            Op::NoteNames => Ok(NoteNameRecord::encode_list(&[])),
            Op::TrackInfo => Ok(Vec::new()),
            Op::PresetLoad => {
                Err("presets of a sandboxed VST3 plugin are not available yet".into())
            }
            Op::OpenEditor => self.open_editor(&String::from_utf8_lossy(payload)),
        }
    }
}

/// Drives the plugin's `process` from the bridge's audio loop.
pub struct Vst3BlockProcessor {
    plugin: Arc<Vst3Adapter>,
    /// The gestures the engine was last told about.
    reported_gestures: [Option<u32>; MAX_GESTURES],
}

// SAFETY: moved once into the audio thread, which is its only user.
unsafe impl Send for Vst3BlockProcessor {}

impl Vst3BlockProcessor {
    pub fn new(plugin: Arc<Vst3Adapter>) -> Self {
        Vst3BlockProcessor {
            plugin,
            reported_gestures: [None; MAX_GESTURES],
        }
    }
}

/// Tells the engine which parameters the user started or stopped dragging in the
/// plugin window since the last block (modulation pauses meanwhile).
fn report_gestures(
    reported: &mut [Option<u32>; MAX_GESTURES],
    edit_state: &EditState,
    reply: &mut ReplyWriter,
) {
    let now = edit_state.active_gestures();
    for native_id in now.iter().flatten() {
        if !reported.contains(&Some(*native_id)) {
            reply.push_edit(WireParam::new(PARAM_GESTURE_BEGIN, 0, *native_id, 0.0));
        }
    }
    for native_id in reported.iter().flatten() {
        if !now.contains(&Some(*native_id)) {
            reply.push_edit(WireParam::new(PARAM_GESTURE_END, 0, *native_id, 0.0));
        }
    }
    *reported = now;
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
        reply: &mut ReplyWriter,
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

        let edit_state = plugin.edit_state();
        activation.param_changes.reset();
        for param in block.params.iter() {
            if param.kind == PARAM_VALUE {
                edit_state.mirror().record(param.native_id, param.value);
                activation
                    .param_changes
                    .push(param.native_id, param.offset as i32, param.value);
            }
        }
        report_gestures(&mut self.reported_gestures, edit_state, reply);
        // The window's own edits go last, so the user's value wins within the block,
        // and are not mirrored back: its controller has them.
        edit_state.drain_edits(|change| {
            let value = f64::from(change.normalised);
            activation
                .param_changes
                .push(change.native_id, change.sample_offset as i32, value);
            reply.push_edit(WireParam::new(PARAM_VALUE, 0, change.native_id, value));
        });
        let param_changes = if activation.param_changes.count > 0 {
            activation.param_changes.as_mut() as *mut HostParameterChanges as *mut c_void
        } else {
            std::ptr::null_mut()
        };
        activation.output_changes.reset();
        let output_changes =
            activation.output_changes.as_mut() as *mut HostParameterChanges as *mut c_void;

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
            output_param_changes: output_changes,
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
        for (native_id, value) in activation.output_changes.latest_points() {
            edit_state.mirror().record(native_id, value);
        }
        for channel in 0..block.channels {
            output[channel * frames..(channel + 1) * frames]
                .copy_from_slice(&activation.output[0].channel(channel)[..frames]);
        }
        plugin.signals.take_reply_flags()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::OnceLock;

    use plugin_bridge::protocol::{write_records, Section, PARAM_BYTES};

    use super::*;

    const SELF_VALUE: u32 = 7;
    const WRITE_OUTPUT: u32 = 8;
    const FRAMES: usize = 32;

    fn test_plugin_dll() -> &'static PathBuf {
        static DLL: OnceLock<PathBuf> = OnceLock::new();
        DLL.get_or_init(|| {
            let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let target = workspace.join("target").join("vst3-bridge-test-plugin");
            let status = Command::new(env!("CARGO"))
                .args(["build", "--quiet", "-p", "test_vst3_plugin", "--target-dir"])
                .arg(&target)
                .current_dir(&workspace)
                .status()
                .expect("run cargo");
            assert!(status.success(), "building the VST3 test plugin failed");
            target.join("debug").join("test_vst3_plugin.dll")
        })
    }

    fn effect_class_id() -> String {
        b"EchoesTestGain_F"
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect()
    }

    /// The effect loaded and activated in this process, with its audio processor.
    fn activated() -> (Arc<Vst3Adapter>, Vst3BlockProcessor) {
        initialise_com_for_the_test();
        let adapter =
            Vst3Adapter::load(test_plugin_dll(), &effect_class_id(), 2).expect("the effect loads");
        adapter
            .activate(48_000.0, 64)
            .expect("the effect activates");
        let processor = Vst3BlockProcessor::new(Arc::clone(&adapter));
        (adapter, processor)
    }

    fn initialise_com_for_the_test() {
        #[cfg(windows)]
        {
            #[link(name = "ole32")]
            extern "system" {
                fn OleInitialize(reserved: *mut std::ffi::c_void) -> i32;
            }
            // SAFETY: once per test thread, before the plugin is loaded.
            unsafe { OleInitialize(std::ptr::null_mut()) };
        }
    }

    /// One block of silence with `params`; returns the reply flags and edits.
    fn run_block(
        processor: &mut Vst3BlockProcessor,
        params: &[WireParam],
    ) -> (u32, Vec<WireParam>) {
        let mut bytes = vec![0u8; params.len() * PARAM_BYTES];
        write_records(params, &mut bytes, 0);
        let block = BlockContext {
            frames: FRAMES,
            channels: 2,
            has_input: true,
            reset: false,
            sequence: 0,
            steady_time: 0,
            transport: None,
            notes: Section::new(&[], 0),
            params: Section::new(&bytes, params.len()),
        };
        let input = vec![0.0f32; FRAMES * 2];
        let mut output = vec![0.0f32; FRAMES * 2];
        let mut edits = [WireParam::new(0, 0, 0, 0.0); 16];
        let mut midi = [];
        let mut reply = ReplyWriter::new(&mut edits, &mut midi);
        let flags = processor.process(&block, &input, &mut output, &mut reply);
        let count = reply.edit_count();
        (flags, edits[..count].to_vec())
    }

    fn controller_value(adapter: &Vst3Adapter, native_id: u32) -> f64 {
        let bytes = adapter.param_value(native_id).expect("a value");
        f64::from_le_bytes(bytes.try_into().expect("eight bytes"))
    }

    fn value(native_id: u32, normalised: f64) -> WireParam {
        WireParam::new(PARAM_VALUE, 0, native_id, normalised)
    }

    #[test]
    fn the_controller_follows_the_processor_only_while_a_window_watches() {
        let (adapter, mut processor) = activated();

        run_block(&mut processor, &[value(SELF_VALUE, 0.7)]);
        adapter.mirror_to_controller();
        assert_eq!(
            controller_value(&adapter, SELF_VALUE),
            0.0,
            "nobody applies the values while no window is open"
        );

        let watch = adapter.watch_params().expect("the window watches");
        run_block(&mut processor, &[value(SELF_VALUE, 0.6)]);
        adapter.mirror_to_controller();
        assert_eq!(controller_value(&adapter, SELF_VALUE), 0.6);

        drop(watch);
        run_block(&mut processor, &[value(SELF_VALUE, 0.3)]);
        adapter.mirror_to_controller();
        assert_eq!(controller_value(&adapter, SELF_VALUE), 0.6, "closed again");
    }

    #[test]
    fn a_value_the_plugin_writes_to_its_output_reaches_the_controller_of_an_open_window() {
        let (adapter, mut processor) = activated();
        let _watch = adapter.watch_params().expect("the window watches");

        let (flags, edits) = run_block(&mut processor, &[value(WRITE_OUTPUT, 1.0)]);
        assert!(
            edits.is_empty(),
            "the processor already has it: not an edit"
        );
        assert_eq!(flags & BLOCK_PLUGIN_EDITED, 0);
        adapter.mirror_to_controller();
        assert_eq!(controller_value(&adapter, SELF_VALUE), 0.9);
    }

    #[test]
    fn dropping_the_adapter_releases_the_controller_before_the_component() {
        let (adapter, processor) = activated();
        drop(processor);
        drop(adapter);
    }
}
