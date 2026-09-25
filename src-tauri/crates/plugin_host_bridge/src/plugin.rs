//! The CLAP plugin hosted by this bridge process.
//!
//! [`LoadedPlugin`] is shared by the main thread (control requests, editor) and
//! the audio thread ([`ClapBlockProcessor`]), following CLAP's thread rules:
//! `activate`/`deactivate`, state, parameters and GUI on the main thread;
//! `start_processing`/`process`/`stop_processing` on the audio thread.

use std::ffi::{c_char, c_void};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap_ffi::{
    read_pod, split_events, ClapAudioBuffer, ClapEventBuffer, ClapEventHeader, ClapEventTransport,
    ClapHost, ClapLibrary, ClapOutputEvents, ClapParamInfo, ClapPlugin, ClapPluginFactory,
    ClapPluginLatency, ClapPluginParams, ClapProcess, CLAP_CORE_EVENT_SPACE, CLAP_EVENT_TRANSPORT,
    CLAP_EXT_LATENCY, CLAP_EXT_PARAMS, CLAP_PLUGIN_FACTORY_ID, CLAP_PROCESS_ERROR,
};
use plugin_bridge::protocol::{BLOCK_PASSED_THROUGH, BLOCK_PLUGIN_EDITED, BLOCK_RESTART_REQUESTED};
use plugin_bridge::session::{BlockContext, BridgeProcessor, EventWriter};

/// How long `activate` waits for the audio thread to stop processing.
const HOLD_TIMEOUT: Duration = Duration::from_secs(2);

pub struct LoadedPlugin {
    plugin: *const ClapPlugin,
    /// Must outlive `plugin` (CLAP: the host pointer given to `create_plugin`).
    host: Box<ClapHost>,
    _library: Arc<ClapLibrary>,
    active: AtomicBool,
    /// Main thread asks the audio thread to stop processing (reconfiguration).
    hold: AtomicBool,
    /// The audio thread has called `start_processing`.
    processing: AtomicBool,
    /// `(sample_rate, max_frames)` of the current activation.  Main thread.
    config: Mutex<Option<(f64, u32)>>,
    /// Held around `process` and around a parameter flush while active, so the
    /// two never overlap.
    process_lock: Mutex<()>,
}

// SAFETY: the plugin is reached from the main and audio threads only through
// the calls CLAP allows on each (see module doc); the atomics and locks above
// sequence the ones that must not overlap.
unsafe impl Send for LoadedPlugin {}
unsafe impl Sync for LoadedPlugin {}

impl LoadedPlugin {
    /// Load the library, create the plugin `id` and `init` it.
    pub fn load(dll: &Path, id: &str) -> Result<Self, String> {
        let (library, entry) = ClapLibrary::load(dll)?;
        // SAFETY: `entry` is the live entry of the just-loaded library.
        let factory =
            unsafe { ((*entry).get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const c_char) }
                as *const ClapPluginFactory;
        if factory.is_null() {
            return Err("clap_entry.get_factory returned null".into());
        }
        let host = clap_ffi::make_host();
        let id_c: Vec<u8> = id.bytes().chain(std::iter::once(0)).collect();
        // SAFETY: live factory and host; NUL-terminated id.
        let plugin = unsafe {
            ((*factory).create_plugin)(factory, host.as_ref(), id_c.as_ptr() as *const c_char)
        };
        if plugin.is_null() {
            return Err(format!("create_plugin returned null for '{id}'"));
        }
        // SAFETY: freshly created plugin; `init` must run once before anything else.
        if !unsafe { ((*plugin).init)(plugin) } {
            unsafe { ((*plugin).destroy)(plugin) };
            return Err(format!("init failed for '{id}'"));
        }
        Ok(LoadedPlugin {
            plugin,
            host,
            _library: library,
            active: AtomicBool::new(false),
            hold: AtomicBool::new(false),
            processing: AtomicBool::new(false),
            config: Mutex::new(None),
            process_lock: Mutex::new(()),
        })
    }

    pub fn plugin(&self) -> *const ClapPlugin {
        self.plugin
    }

    pub fn host(&self) -> &ClapHost {
        &self.host
    }

    /// Plugin extension `id`, or `None` when the plugin does not provide it.
    pub fn extension<T>(&self, id: &[u8]) -> Option<*const T> {
        // SAFETY: `get_extension` is callable from any thread.
        let pointer =
            unsafe { ((*self.plugin).get_extension)(self.plugin, id.as_ptr() as *const c_char) };
        (!pointer.is_null()).then_some(pointer as *const T)
    }

    /// Main thread.  Activates at `(sample_rate, max_frames)` — a no-op when
    /// already active there, unless the plugin requested a restart — and
    /// returns the plugin's latency.
    pub fn activate(&self, sample_rate: f64, max_frames: u32) -> Result<u32, String> {
        let mut config = self.config.lock().unwrap_or_else(|e| e.into_inner());
        let restart = self.host.context().is_some_and(|context| context.take_restart_request());
        if restart || *config != Some((sample_rate, max_frames)) {
            *config = None;
            self.reactivate(sample_rate, max_frames)?;
            *config = Some((sample_rate, max_frames));
        }
        Ok(self.latency())
    }

    fn reactivate(&self, sample_rate: f64, max_frames: u32) -> Result<(), String> {
        self.hold.store(true, Ordering::Release);
        let deadline = Instant::now() + HOLD_TIMEOUT;
        while self.processing.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                self.hold.store(false, Ordering::Release);
                return Err("the audio thread did not stop processing".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let _no_process = self.process_lock.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: main thread, processing stopped.
        unsafe {
            if self.active.swap(false, Ordering::AcqRel) {
                ((*self.plugin).deactivate)(self.plugin);
            }
            if !((*self.plugin).activate)(self.plugin, sample_rate, 1, max_frames) {
                self.hold.store(false, Ordering::Release);
                return Err("activate failed".into());
            }
        }
        self.active.store(true, Ordering::Release);
        self.hold.store(false, Ordering::Release);
        Ok(())
    }

    fn latency(&self) -> u32 {
        self.extension::<ClapPluginLatency>(CLAP_EXT_LATENCY)
            // SAFETY: main thread, after activate.
            .map_or(0, |latency| unsafe { ((*latency).get)(self.plugin) })
    }

    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        // SAFETY: main thread.
        unsafe { clap_ffi::read_plugin_state(self.plugin) }
            .ok_or_else(|| "the plugin has no state".into())
    }

    pub fn load_state(&self, bytes: &[u8]) -> Result<(), String> {
        // SAFETY: main thread.
        if unsafe { clap_ffi::write_plugin_state(self.plugin, bytes) } {
            Ok(())
        } else {
            Err("the plugin rejected the state".into())
        }
    }

    /// `count:u32` followed by each raw `clap_param_info`.
    pub fn params(&self) -> Vec<u8> {
        let Some(params) = self.extension::<ClapPluginParams>(CLAP_EXT_PARAMS) else {
            return 0u32.to_le_bytes().to_vec();
        };
        let mut infos = Vec::new();
        // SAFETY: main thread; `info` is written by the plugin before use.
        unsafe {
            for index in 0..((*params).count)(self.plugin) {
                let mut info: ClapParamInfo = std::mem::zeroed();
                if ((*params).get_info)(self.plugin, index, &mut info) {
                    info.cookie = std::ptr::null_mut();
                    infos.push(info);
                }
            }
        }
        let mut bytes = (infos.len() as u32).to_le_bytes().to_vec();
        for info in &infos {
            // SAFETY: `ClapParamInfo` is POD.
            bytes.extend_from_slice(unsafe {
                std::slice::from_raw_parts(
                    info as *const ClapParamInfo as *const u8,
                    std::mem::size_of::<ClapParamInfo>(),
                )
            });
        }
        bytes
    }

    pub fn param_value(&self, native_id: u32) -> Result<f64, String> {
        let params = self
            .extension::<ClapPluginParams>(CLAP_EXT_PARAMS)
            .ok_or("the plugin has no parameters")?;
        let mut value = 0.0;
        // SAFETY: main thread.
        if unsafe { ((*params).get_value)(self.plugin, native_id, &mut value) } {
            Ok(value)
        } else {
            Err(format!("unknown parameter {native_id}"))
        }
    }

    /// Sets plain parameter values outside `process` (`clap.params.flush`).
    pub fn flush_values(&self, values: &[(u32, f64)]) {
        let Some(params) = self.extension::<ClapPluginParams>(CLAP_EXT_PARAMS) else {
            return;
        };
        let mut events = ClapEventBuffer::new();
        for &(native_id, value) in values {
            events.push_param_value(0, native_id, value);
        }
        let output = ClapOutputEvents {
            ctx: std::ptr::null_mut(),
            try_push: discard_output_event,
        };
        let _no_process = self.process_lock.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `process` cannot run concurrently (lock above).
        unsafe { ((*params).flush)(self.plugin, &events.input_events_vtable, &output) };
    }
}

impl Drop for LoadedPlugin {
    // Runs on the main thread after the audio thread has exited.
    fn drop(&mut self) {
        // SAFETY: CLAP teardown order; no other thread uses the plugin any more.
        unsafe {
            if self.processing.load(Ordering::Acquire) {
                ((*self.plugin).stop_processing)(self.plugin);
            }
            if self.active.load(Ordering::Acquire) {
                ((*self.plugin).deactivate)(self.plugin);
            }
            ((*self.plugin).destroy)(self.plugin);
        }
    }
}

unsafe extern "C" fn discard_output_event(
    _: *const ClapOutputEvents,
    _: *const ClapEventHeader,
) -> bool {
    true
}

/// Copies each plugin output event into the block's [`EventWriter`].
unsafe extern "C" fn write_output_event(
    list: *const ClapOutputEvents,
    header: *const ClapEventHeader,
) -> bool {
    let writer = (*list).ctx as *mut EventWriter;
    !writer.is_null() && !header.is_null() && (*writer).push(clap_ffi::event_bytes(&*header))
}

/// Drives the plugin's `process` from the bridge's audio loop.
pub struct ClapBlockProcessor {
    plugin: Arc<LoadedPlugin>,
    events: Box<ClapEventBuffer>,
    input_pointers: Vec<*mut f32>,
    output_pointers: Vec<*mut f32>,
}

// SAFETY: moved once into the audio thread, which is its only user.
unsafe impl Send for ClapBlockProcessor {}

impl ClapBlockProcessor {
    pub fn new(plugin: Arc<LoadedPlugin>, channels: usize) -> Self {
        ClapBlockProcessor {
            plugin,
            events: ClapEventBuffer::new(),
            input_pointers: vec![std::ptr::null_mut(); channels],
            output_pointers: vec![std::ptr::null_mut(); channels],
        }
    }

    /// Stops processing if the main thread asked for it or the plugin is not
    /// active.  Returns whether the plugin may process now.
    fn ready_to_process(&mut self) -> bool {
        let plugin = &self.plugin;
        let may_run = plugin.active.load(Ordering::Acquire) && !plugin.hold.load(Ordering::Acquire);
        let processing = plugin.processing.load(Ordering::Acquire);
        // SAFETY: audio thread — the thread CLAP requires for these two calls.
        unsafe {
            if !may_run {
                if processing {
                    ((*plugin.plugin).stop_processing)(plugin.plugin);
                    plugin.processing.store(false, Ordering::Release);
                }
                return false;
            }
            if !processing {
                if !((*plugin.plugin).start_processing)(plugin.plugin) {
                    return false;
                }
                plugin.processing.store(true, Ordering::Release);
            }
        }
        true
    }

    /// Fills the event buffer from the engine's events and returns its transport.
    fn read_events(&mut self, bytes: &[u8]) -> Option<ClapEventTransport> {
        self.events.clear();
        let mut transport = None;
        for event in split_events(bytes) {
            match read_pod::<ClapEventHeader>(event) {
                Some(header)
                    if header.space_id == CLAP_CORE_EVENT_SPACE
                        && header.r#type == CLAP_EVENT_TRANSPORT =>
                {
                    transport = read_pod::<ClapEventTransport>(event);
                }
                _ => self.events.push_bytes(event),
            }
        }
        self.events.sort();
        transport
    }
}

/// Reports to the engine what the plugin signalled since the last block.  This
/// process hosts a single plugin, so the process-wide flags are its own.
fn signal_flags() -> u32 {
    let mut flags = 0;
    if clap_ffi::take_plugin_state_edited() {
        flags |= BLOCK_PLUGIN_EDITED;
    }
    if clap_ffi::take_plugin_restart_requested() {
        flags |= BLOCK_RESTART_REQUESTED;
    }
    flags
}

impl BridgeProcessor for ClapBlockProcessor {
    fn process(
        &mut self,
        block: &BlockContext,
        input: &[f32],
        output: &mut [f32],
        out_events: &mut EventWriter,
    ) -> u32 {
        if !self.ready_to_process() {
            output.copy_from_slice(input);
            return BLOCK_PASSED_THROUGH | signal_flags();
        }
        if block.reset {
            // SAFETY: audio thread, plugin active and processing.
            unsafe { ((*self.plugin.plugin).reset)(self.plugin.plugin) };
        }
        let transport = self.read_events(block.events);
        let frames = block.frames;
        for channel in 0..block.channels {
            self.input_pointers[channel] = input[channel * frames..].as_ptr() as *mut f32;
            self.output_pointers[channel] = output[channel * frames..].as_mut_ptr();
        }
        let input_buffer = ClapAudioBuffer {
            data32: self.input_pointers.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: block.channels as u32,
            latency: 0,
            constant_mask: 0,
        };
        let mut output_buffer = ClapAudioBuffer {
            data32: self.output_pointers.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: block.channels as u32,
            latency: 0,
            constant_mask: 0,
        };
        let output_events = ClapOutputEvents {
            ctx: out_events as *mut EventWriter as *mut c_void,
            try_push: write_output_event,
        };
        let process = ClapProcess {
            steady_time: block.steady_time,
            frames_count: frames as u32,
            transport: transport
                .as_ref()
                .map_or(std::ptr::null(), |t| t as *const ClapEventTransport),
            audio_inputs: &input_buffer,
            audio_outputs: &mut output_buffer,
            audio_inputs_count: u32::from(block.has_input),
            audio_outputs_count: 1,
            in_events: &self.events.input_events_vtable,
            out_events: &output_events,
        };

        let status = {
            let _exclusive = self
                .plugin
                .process_lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // SAFETY: audio thread, plugin processing; every buffer outlives the call.
            unsafe { ((*self.plugin.plugin).process)(self.plugin.plugin, &process) }
        };
        if status == CLAP_PROCESS_ERROR {
            output.copy_from_slice(input);
            return BLOCK_PASSED_THROUGH | signal_flags();
        }
        signal_flags()
    }

    fn idle(&mut self) {
        let plugin = &self.plugin;
        if plugin.hold.load(Ordering::Acquire) || !plugin.active.load(Ordering::Acquire) {
            self.ready_to_process();
        }
    }
}
