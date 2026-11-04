//! The CLAP adapter: the plugin hosted by this bridge process, seen through the
//! format-neutral protocol (ADR-033).
//!
//! [`ClapAdapter`] is shared by the main thread (control requests, editor) and
//! the audio thread ([`ClapBlockProcessor`]), following CLAP's thread rules:
//! `activate`/`deactivate`, state, parameters and GUI on the main thread;
//! `start_processing`/`process`/`stop_processing` on the audio thread.  The
//! engine never sends a CLAP structure: the adapter builds the CLAP events from
//! the block's sections and turns the plugin's output events back into edits.

use std::ffi::{c_char, c_void, CStr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap_ffi::{
    audio_thread_scope, deliver_due_timers, is_silent, read_tail, transport_event, BlockActivity,
    ClapAudioBuffer, ClapAudioPortInfo, ClapEventBuffer, ClapEventHeader, ClapEventMidi,
    ClapEventNote, ClapEventParamGesture, ClapEventParamValue, ClapEventTransport, ClapHost,
    ClapLibrary, ClapOutputEvents, ClapParamInfo, ClapPlugin, ClapPluginAudioPorts,
    ClapPluginFactory, ClapPluginLatency, ClapPluginParams, ClapProcess, HostTrackInfo,
    PresetRequest, ProcessSleep, TimerSchedule, TransportValues, CLAP_AUDIO_PORT_IS_MAIN,
    CLAP_CORE_EVENT_SPACE, CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON,
    CLAP_EVENT_PARAM_GESTURE_BEGIN, CLAP_EVENT_PARAM_GESTURE_END, CLAP_EVENT_PARAM_VALUE,
    CLAP_EXT_AUDIO_PORTS, CLAP_EXT_LATENCY, CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE,
    CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_MODULATABLE, CLAP_PARAM_IS_READONLY, CLAP_PARAM_RESCAN_ALL,
    CLAP_PARAM_RESCAN_INFO, CLAP_PARAM_RESCAN_TEXT, CLAP_PARAM_RESCAN_VALUES,
    CLAP_PLUGIN_FACTORY_ID, CLAP_PRESET_LOCATION_FILE, CLAP_PRESET_LOCATION_PLUGIN,
    CLAP_PROCESS_ERROR,
};
use plugin_bridge::protocol::{
    WireNote, WireParam, BLOCK_NOTE_NAMES_CHANGED, BLOCK_PARAMS_RESCAN_MASK,
    BLOCK_PARAMS_RESCAN_SHIFT, BLOCK_PASSED_THROUGH, BLOCK_PLUGIN_EDITED, BLOCK_RESTART_REQUESTED,
    MIDI, NOTE_OFF, NOTE_ON, PARAMS_INFO, PARAMS_LIST, PARAMS_TEXT, PARAMS_VALUES,
    PARAM_GESTURE_BEGIN, PARAM_GESTURE_END, PARAM_MODULATION, PARAM_VALUE, TRANSPORT_PLAYING,
};
use plugin_bridge::records::{
    AudioPortRecord, Handshake, NoteNameRecord, ParamRecord, ParamUnit, PortDirection, PortRole,
    PresetRecord, PresetSource, SpeakerLayout, PARAM_AUTOMATABLE, PARAM_HIDDEN, PARAM_MODULATABLE,
};
use plugin_bridge::session::{BlockContext, BridgeProcessor, ReplyWriter};

/// How long `activate` waits for the audio thread to stop processing.
const HOLD_TIMEOUT: Duration = Duration::from_secs(2);

/// Stored in [`ClapAdapter::tail`] when the plugin has no `clap.tail`.
const NO_TAIL: u32 = u32::MAX;

pub struct ClapAdapter {
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
    /// The plugin's `clap.tail` in samples, [`NO_TAIL`] when it has none.
    tail: AtomicU32,
}

// SAFETY: the plugin is reached from the main and audio threads only through
// the calls CLAP allows on each (see module doc); the atomics and locks above
// sequence the ones that must not overlap.
unsafe impl Send for ClapAdapter {}
unsafe impl Sync for ClapAdapter {}

impl ClapAdapter {
    /// Load the library, create the plugin `id` and `init` it.  `track_info` is
    /// what `clap.track-info` answers from `init` on.
    pub fn load(dll: &Path, id: &str, track_info: Option<&HostTrackInfo>) -> Result<Self, String> {
        let (library, entry) = ClapLibrary::load(dll)?;
        // SAFETY: `entry` is the live entry of the just-loaded library.
        let factory =
            unsafe { ((*entry).get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const c_char) }
                as *const ClapPluginFactory;
        if factory.is_null() {
            return Err("clap_entry.get_factory returned null".into());
        }
        let host = clap_ffi::make_host();
        if let (Some(context), Some(info)) = (host.context(), track_info) {
            context.set_track_info(info);
        }
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
        Ok(ClapAdapter {
            plugin,
            host,
            _library: library,
            active: AtomicBool::new(false),
            hold: AtomicBool::new(false),
            processing: AtomicBool::new(false),
            config: Mutex::new(None),
            process_lock: Mutex::new(()),
            tail: AtomicU32::new(NO_TAIL),
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

    /// The unit of the plugin's parameter values and its audio ports.  The first
    /// main (else first) port of each direction is the active one: it is the bus
    /// the audio loop binds.
    pub fn handshake(&self) -> Vec<u8> {
        let mut ports = Vec::new();
        let extension = self.extension::<ClapPluginAudioPorts>(CLAP_EXT_AUDIO_PORTS);
        if extension.is_none() {
            // Without `clap.audio-ports` the plugin is driven on one stereo bus each way.
            ports.extend(
                [PortDirection::Input, PortDirection::Output].map(|direction| AudioPortRecord {
                    direction,
                    role: PortRole::Main,
                    channels: 2,
                    layout: SpeakerLayout::Stereo,
                    active: true,
                }),
            );
        }
        if let Some(extension) = extension {
            for (is_input, direction) in
                [(true, PortDirection::Input), (false, PortDirection::Output)]
            {
                // SAFETY: main thread, after `init`.
                let mut declared = unsafe { read_ports(self.plugin, extension, is_input) };
                let active = declared
                    .iter()
                    .position(|(main, _)| *main)
                    .or_else(|| (!declared.is_empty()).then_some(0));
                for (index, (main, channels)) in declared.drain(..).enumerate() {
                    ports.push(AudioPortRecord {
                        direction,
                        role: if main {
                            PortRole::Main
                        } else {
                            PortRole::Auxiliary
                        },
                        channels,
                        layout: match channels {
                            1 => SpeakerLayout::Mono,
                            2 => SpeakerLayout::Stereo,
                            _ => SpeakerLayout::Other,
                        },
                        active: Some(index) == active,
                    });
                }
            }
        }
        Handshake {
            param_unit: ParamUnit::Plain,
            ports,
        }
        .encode()
    }

    /// Main thread.  Activates at `(sample_rate, max_frames)` — a no-op when
    /// already active there, unless the plugin requested a restart — and
    /// returns the plugin's latency.
    pub fn activate(&self, sample_rate: f64, max_frames: u32) -> Result<u32, String> {
        let mut config = self.config.lock().unwrap_or_else(|e| e.into_inner());
        let restart = self
            .host
            .context()
            .is_some_and(|context| context.take_restart_request());
        if restart || *config != Some((sample_rate, max_frames)) {
            *config = None;
            self.reactivate(sample_rate, max_frames)?;
            *config = Some((sample_rate, max_frames));
        }
        // SAFETY: main thread, after activate.
        let tail = unsafe { read_tail(self.plugin) };
        self.tail.store(tail.unwrap_or(NO_TAIL), Ordering::Release);
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

    /// Main thread.  The plugin loads one preset (`clap.preset-load`), beside
    /// `process` as CLAP allows, like `load_state`; a sleeping plugin is woken.
    pub fn load_preset(&self, request: &[u8]) -> Result<(), String> {
        let record = PresetRecord::decode(request).map_err(|_| "invalid preset request")?;
        let request = PresetRequest {
            location_kind: match record.source {
                PresetSource::File => CLAP_PRESET_LOCATION_FILE,
                PresetSource::Plugin => CLAP_PRESET_LOCATION_PLUGIN,
            },
            location: record.location,
            load_key: record.load_key,
        };
        let context = self.host.context();
        if let Some(context) = context {
            context.take_preset_error();
        }
        // SAFETY: main thread.
        if !unsafe { clap_ffi::load_preset(self.plugin, &request) } {
            return Err(context
                .and_then(|context| context.take_preset_error())
                .unwrap_or_else(|| "the plugin could not load this preset".into()));
        }
        if let Some(context) = context {
            context.request_process();
        }
        Ok(())
    }

    /// The plugin's parameters as [`ParamRecord`]s, in the plugin's own plain units.
    pub fn params(&self) -> Vec<u8> {
        let Some(params) = self.extension::<ClapPluginParams>(CLAP_EXT_PARAMS) else {
            return ParamRecord::encode_list(&[]);
        };
        let mut records = Vec::new();
        // SAFETY: main thread; `info` is written by the plugin before use.
        unsafe {
            for index in 0..((*params).count)(self.plugin) {
                let mut info: ClapParamInfo = std::mem::zeroed();
                if ((*params).get_info)(self.plugin, index, &mut info) {
                    records.push(param_record(&info));
                }
            }
        }
        ParamRecord::encode_list(&records)
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

    /// The plugin's text for a plain parameter value; empty when it has none.
    pub fn param_text(&self, native_id: u32, value: f64) -> Vec<u8> {
        // SAFETY: main thread; `value_to_text` may run beside `process`.
        unsafe { clap_ffi::read_param_text(self.plugin, native_id, value) }
            .map(String::into_bytes)
            .unwrap_or_default()
    }

    /// The plugin's note names as [`NoteNameRecord`]s; empty when it has none.
    pub fn note_names(&self) -> Vec<u8> {
        // SAFETY: main thread.
        let names = unsafe { clap_ffi::read_note_names(self.plugin) };
        let records: Vec<NoteNameRecord> = names
            .into_iter()
            .map(|name| NoteNameRecord {
                name: name.name,
                port: name.port,
                key: name.key,
                channel: name.channel,
            })
            .collect();
        NoteNameRecord::encode_list(&records)
    }

    /// Main thread.  Records the plugin's track and, when it changed, tells the
    /// plugin; `process` is not held back, CLAP lets the two run together.
    pub fn set_track_info(&self, info: &HostTrackInfo) {
        let Some(context) = self.host.context() else {
            return;
        };
        if context.set_track_info(info) {
            // SAFETY: live plugin, called on the bridge's main thread.
            unsafe { clap_ffi::notify_track_info_changed(self.plugin) };
        }
    }

    /// Main thread.  Calls the `clap.timer-support` `on_timer` of every timer due
    /// at `now`, only while the plugin's editor is open (a closed editor starts
    /// `schedule` over, so its timers do not catch up).  Returns the number of
    /// calls.  The control requests run on this same thread, so no off-RT call
    /// can overlap one; `process` may, as CLAP allows.
    pub fn run_due_timers(&self, schedule: &mut TimerSchedule, now: Instant) -> usize {
        let Some(context) = self.host.context() else {
            return 0;
        };
        if !context.editor_open() {
            *schedule = TimerSchedule::default();
            return 0;
        }
        // SAFETY: live plugin, called on the bridge's main thread.
        unsafe { deliver_due_timers(self.plugin, context, schedule, now, || Some(())) }
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

impl Drop for ClapAdapter {
    // Runs on the main thread after the audio thread has exited.
    fn drop(&mut self) {
        // SAFETY: CLAP teardown order; no other thread uses the plugin any more.
        unsafe {
            if self.processing.load(Ordering::Acquire) {
                // The audio thread has exited, so this thread takes its role.
                let _audio_thread = audio_thread_scope();
                ((*self.plugin).stop_processing)(self.plugin);
            }
            if self.active.load(Ordering::Acquire) {
                ((*self.plugin).deactivate)(self.plugin);
            }
            ((*self.plugin).destroy)(self.plugin);
        }
    }
}

fn c_text(bytes: &[u8]) -> String {
    CStr::from_bytes_until_nul(bytes)
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A read-only parameter is neither automatable nor modulatable.
fn param_record(info: &ClapParamInfo) -> ParamRecord {
    let usable = info.flags & CLAP_PARAM_IS_READONLY == 0;
    let mut flags = 0;
    if usable && info.flags & CLAP_PARAM_IS_AUTOMATABLE != 0 {
        flags |= PARAM_AUTOMATABLE;
    }
    if usable && info.flags & CLAP_PARAM_IS_MODULATABLE != 0 {
        flags |= PARAM_MODULATABLE;
    }
    if info.flags & CLAP_PARAM_IS_HIDDEN != 0 {
        flags |= PARAM_HIDDEN;
    }
    ParamRecord {
        native_id: info.id,
        name: c_text(&info.name),
        module: c_text(&info.module),
        min: info.min_value,
        max: info.max_value,
        default: info.default_value,
        flags,
    }
}

/// `(is main, channel count)` of each port of one direction.
///
/// # Safety
/// `plugin` is live and `init`ed, `extension` is its `clap.audio-ports`, and this
/// is the main thread.
unsafe fn read_ports(
    plugin: *const ClapPlugin,
    extension: *const ClapPluginAudioPorts,
    is_input: bool,
) -> Vec<(bool, u32)> {
    (0..((*extension).count)(plugin, is_input))
        .filter_map(|index| {
            let mut info: ClapAudioPortInfo = std::mem::zeroed();
            ((*extension).get)(plugin, index, is_input, &mut info).then_some((
                info.flags & CLAP_AUDIO_PORT_IS_MAIN != 0,
                info.channel_count,
            ))
        })
        .collect()
}

unsafe extern "C" fn discard_output_event(
    _: *const ClapOutputEvents,
    _: *const ClapEventHeader,
) -> bool {
    true
}

/// Turns each plugin output event into an edit or a MIDI message of the block's
/// [`ReplyWriter`]; any other event is dropped.
unsafe extern "C" fn write_output_event(
    list: *const ClapOutputEvents,
    header: *const ClapEventHeader,
) -> bool {
    let reply = (*list).ctx as *mut ReplyWriter;
    if reply.is_null() || header.is_null() || (*header).space_id != CLAP_CORE_EVENT_SPACE {
        return false;
    }
    let reply = &mut *reply;
    let header = &*header;
    match header.r#type {
        CLAP_EVENT_PARAM_VALUE => {
            let event = &*(header as *const ClapEventHeader as *const ClapEventParamValue);
            reply.push_edit(WireParam::new(
                PARAM_VALUE,
                header.time,
                event.param_id,
                event.value,
            ))
        }
        CLAP_EVENT_PARAM_GESTURE_BEGIN | CLAP_EVENT_PARAM_GESTURE_END => {
            let event = &*(header as *const ClapEventHeader as *const ClapEventParamGesture);
            let kind = if header.r#type == CLAP_EVENT_PARAM_GESTURE_BEGIN {
                PARAM_GESTURE_BEGIN
            } else {
                PARAM_GESTURE_END
            };
            reply.push_edit(WireParam::new(kind, header.time, event.param_id, 0.0))
        }
        CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => {
            let event = &*(header as *const ClapEventHeader as *const ClapEventNote);
            let (kind, velocity) = if header.r#type == CLAP_EVENT_NOTE_ON {
                (
                    NOTE_ON,
                    (event.velocity.clamp(0.0, 1.0) * 127.0).round() as u8,
                )
            } else {
                (NOTE_OFF, 0)
            };
            let mut note = WireNote::new(header.time, kind, [event.key as u8, velocity, 0]);
            note.channel = event.channel.clamp(0, 15) as u8;
            reply.push_midi(note)
        }
        CLAP_EVENT_MIDI => {
            let event = &*(header as *const ClapEventHeader as *const ClapEventMidi);
            reply.push_midi(WireNote::new(header.time, MIDI, event.data))
        }
        _ => true,
    }
}

/// Drives the plugin's `process` from the bridge's audio loop.
pub struct ClapBlockProcessor {
    plugin: Arc<ClapAdapter>,
    events: Box<ClapEventBuffer>,
    input_pointers: Vec<*mut f32>,
    output_pointers: Vec<*mut f32>,
    /// Stops calling the plugin while it is silent and has no tail left.  The
    /// engine keeps exchanging every block with the bridge, so its bounded
    /// wait and the offline render are unchanged.
    sleep: ProcessSleep,
    applied_tail: u32,
}

// SAFETY: moved once into the audio thread, which is its only user.
unsafe impl Send for ClapBlockProcessor {}

impl ClapBlockProcessor {
    pub fn new(plugin: Arc<ClapAdapter>, channels: usize) -> Self {
        ClapBlockProcessor {
            plugin,
            events: ClapEventBuffer::new(),
            input_pointers: vec![std::ptr::null_mut(); channels],
            output_pointers: vec![std::ptr::null_mut(); channels],
            sleep: ProcessSleep::new(),
            applied_tail: NO_TAIL,
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
                self.sleep.wake();
            }
        }
        true
    }

    /// Whether this block can skip `process`: the plugin is asleep and nothing
    /// reached it.  `playing` comes from the engine's transport event.
    fn skip_block(&mut self, block: &BlockContext, input: &[f32], playing: bool) -> bool {
        let plugin = &self.plugin;
        let context = plugin.host().context();
        if context.is_some_and(|context| context.take_tail_changed()) {
            // SAFETY: audio thread, plugin active; `clap.tail` is audio-thread callable.
            let tail = unsafe { read_tail(plugin.plugin) };
            plugin
                .tail
                .store(tail.unwrap_or(NO_TAIL), Ordering::Release);
        }
        let tail = plugin.tail.load(Ordering::Acquire);
        if tail != self.applied_tail {
            self.sleep.set_tail(Some(tail));
            self.applied_tail = tail;
        }
        let samples = block.frames * block.channels;
        let input_audible = self.sleep.wants_input_scan()
            && block.has_input
            && !is_silent(&input[..samples.min(input.len())]);
        self.sleep.begin_block(BlockActivity {
            frames: block.frames as u32,
            has_events: !self.events.is_empty(),
            input_audible,
            playing,
            process_requested: context.is_some_and(|context| context.take_process_request()),
        })
    }

    /// Fills the event buffer from the block's sections — parameter events first,
    /// then notes, then one stable sort by time, exactly as the engine does for a
    /// plugin in its own process — and returns the transport event.
    fn read_events(&mut self, block: &BlockContext) -> Option<ClapEventTransport> {
        self.events.clear();
        for param in block.params.iter() {
            match param.kind {
                PARAM_VALUE => {
                    self.events
                        .push_param_value(param.offset, param.native_id, param.value)
                }
                PARAM_MODULATION => {
                    self.events
                        .push_param_mod(param.offset, param.native_id, param.value)
                }
                // CLAP has no host-to-plugin gesture.
                _ => {}
            }
        }
        for note in block.notes.iter() {
            let channel = note.channel as i16;
            let [first, second, _] = note.data;
            match note.kind {
                NOTE_ON => self.events.push_note_on(
                    note.offset,
                    channel,
                    first as i16,
                    second as f64 / 127.0,
                ),
                NOTE_OFF => self
                    .events
                    .push_note_off(note.offset, channel, first as i16),
                MIDI => self.events.push_midi(note.offset, note.data),
                _ => {}
            }
        }
        self.events.sort();
        block.transport.map(|transport| {
            transport_event(&TransportValues {
                playing: transport.flags & TRANSPORT_PLAYING != 0,
                position_beats: transport.position_beats,
                position_seconds: transport.position_seconds,
                tempo: transport.tempo,
                bar_start_beats: transport.bar_start_beats,
                bar_number: transport.bar_number,
                time_signature_numerator: transport.time_signature_numerator,
                time_signature_denominator: transport.time_signature_denominator,
            })
        })
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
    if clap_ffi::take_plugin_note_names_changed() {
        flags |= BLOCK_NOTE_NAMES_CHANGED;
    }
    let rescan = params_changes(clap_ffi::take_plugin_params_rescan()) << BLOCK_PARAMS_RESCAN_SHIFT;
    flags | (rescan & BLOCK_PARAMS_RESCAN_MASK)
}

/// The protocol's `PARAMS_*` bits for a set of `CLAP_PARAM_RESCAN_*` flags.
fn params_changes(rescan: u32) -> u32 {
    [
        (CLAP_PARAM_RESCAN_VALUES, PARAMS_VALUES),
        (CLAP_PARAM_RESCAN_TEXT, PARAMS_TEXT),
        (CLAP_PARAM_RESCAN_INFO, PARAMS_INFO),
        (CLAP_PARAM_RESCAN_ALL, PARAMS_LIST),
    ]
    .into_iter()
    .filter(|(clap, _)| rescan & clap != 0)
    .fold(0, |bits, (_, neutral)| bits | neutral)
}

impl BridgeProcessor for ClapBlockProcessor {
    fn process(
        &mut self,
        block: &BlockContext,
        input: &[f32],
        output: &mut [f32],
        reply: &mut ReplyWriter,
    ) -> u32 {
        let _audio_thread = audio_thread_scope();
        if !self.ready_to_process() {
            output.copy_from_slice(input);
            return BLOCK_PASSED_THROUGH | signal_flags();
        }
        if block.reset {
            // SAFETY: audio thread, plugin active and processing.
            unsafe { ((*self.plugin.plugin).reset)(self.plugin.plugin) };
            self.sleep.wake();
        }
        let transport = self.read_events(block);
        let playing = block
            .transport
            .is_some_and(|t| t.flags & TRANSPORT_PLAYING != 0);
        if self.skip_block(block, input, playing) {
            output.fill(0.0);
            return signal_flags();
        }
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
            ctx: reply as *mut ReplyWriter as *mut c_void,
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
        self.sleep.end_block(status, || is_silent(output));
        if status == CLAP_PROCESS_ERROR {
            output.copy_from_slice(input);
            return BLOCK_PASSED_THROUGH | signal_flags();
        }
        signal_flags()
    }

    fn idle(&mut self) {
        let plugin = &self.plugin;
        if plugin.hold.load(Ordering::Acquire) || !plugin.active.load(Ordering::Acquire) {
            let _audio_thread = audio_thread_scope();
            self.ready_to_process();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap_ffi::CLAP_PARAM_RESCAN_ALL as RESCAN_ALL;

    fn header(r#type: u16, size: usize, time: u32) -> ClapEventHeader {
        ClapEventHeader {
            size: size as u32,
            time,
            space_id: CLAP_CORE_EVENT_SPACE,
            r#type,
            flags: 0,
        }
    }

    fn push_through(header: &ClapEventHeader, reply: &mut ReplyWriter) -> bool {
        let output = ClapOutputEvents {
            ctx: reply as *mut ReplyWriter as *mut c_void,
            try_push: write_output_event,
        };
        // SAFETY: `header` starts a complete event of the type it names.
        unsafe { write_output_event(&output, header) }
    }

    fn writer<'a>(edits: &'a mut [WireParam], midi: &'a mut [WireNote]) -> ReplyWriter<'a> {
        ReplyWriter::new(edits, midi)
    }

    #[test]
    fn the_plugins_output_events_become_edits_and_midi() {
        let mut edits = [WireParam::new(0, 0, 0, 0.0); 4];
        let mut midi = [WireNote::new(0, 0, [0; 3]); 4];
        let mut reply = writer(&mut edits, &mut midi);

        let value = ClapEventParamValue {
            header: header(CLAP_EVENT_PARAM_VALUE, size_of::<ClapEventParamValue>(), 7),
            param_id: 3,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            _pad: 0,
            value: 0.25,
        };
        assert!(push_through(&value.header, &mut reply));
        let gesture = ClapEventParamGesture {
            header: header(
                CLAP_EVENT_PARAM_GESTURE_END,
                size_of::<ClapEventParamGesture>(),
                9,
            ),
            param_id: 3,
        };
        assert!(push_through(&gesture.header, &mut reply));
        let note = ClapEventNote {
            header: header(CLAP_EVENT_NOTE_ON, size_of::<ClapEventNote>(), 2),
            note_id: -1,
            port_index: 0,
            channel: 5,
            key: 64,
            _pad: 0,
            velocity: 100.0 / 127.0,
        };
        assert!(push_through(&note.header, &mut reply));
        let raw = ClapEventMidi {
            header: header(CLAP_EVENT_MIDI, size_of::<ClapEventMidi>(), 4),
            port_index: 0,
            data: [0xB0, 1, 99],
            _pad: [0; 3],
        };
        assert!(push_through(&raw.header, &mut reply));
        let unknown = header(0x7777, size_of::<ClapEventHeader>(), 0);
        assert!(push_through(&unknown, &mut reply), "ignored, not refused");

        let (edit_count, midi_count) = (reply.edit_count(), reply.midi_count());
        assert_eq!((edit_count, midi_count), (2, 2));
        assert_eq!(edits[0], WireParam::new(PARAM_VALUE, 7, 3, 0.25));
        assert_eq!(edits[1], WireParam::new(PARAM_GESTURE_END, 9, 3, 0.0));
        let mut expected_note = WireNote::new(2, NOTE_ON, [64, 100, 0]);
        expected_note.channel = 5;
        assert_eq!(midi[0], expected_note);
        assert_eq!(midi[1], WireNote::new(4, MIDI, [0xB0, 1, 99]));
    }

    #[test]
    fn a_full_reply_refuses_the_event() {
        let mut edits: [WireParam; 0] = [];
        let mut midi: [WireNote; 0] = [];
        let mut reply = writer(&mut edits, &mut midi);
        let gesture = ClapEventParamGesture {
            header: header(
                CLAP_EVENT_PARAM_GESTURE_BEGIN,
                size_of::<ClapEventParamGesture>(),
                0,
            ),
            param_id: 1,
        };
        assert!(!push_through(&gesture.header, &mut reply));
    }

    #[test]
    fn a_read_only_parameter_is_neither_automatable_nor_modulatable() {
        let mut info: ClapParamInfo = unsafe { std::mem::zeroed() };
        info.id = 12;
        info.flags = CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_MODULATABLE | CLAP_PARAM_IS_READONLY;
        assert_eq!(param_record(&info).flags, 0);

        info.flags = CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_HIDDEN;
        assert_eq!(param_record(&info).flags, PARAM_AUTOMATABLE | PARAM_HIDDEN);
        info.flags = CLAP_PARAM_IS_MODULATABLE;
        assert_eq!(param_record(&info).flags, PARAM_MODULATABLE);
    }

    #[test]
    fn rescan_flags_map_to_the_protocol_bits() {
        assert_eq!(params_changes(0), 0);
        assert_eq!(
            params_changes(CLAP_PARAM_RESCAN_TEXT | CLAP_PARAM_RESCAN_INFO),
            PARAMS_TEXT | PARAMS_INFO
        );
        assert_eq!(
            params_changes(RESCAN_ALL | CLAP_PARAM_RESCAN_VALUES),
            PARAMS_LIST | PARAMS_VALUES
        );
    }
}
