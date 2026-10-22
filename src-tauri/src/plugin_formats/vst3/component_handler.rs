//! The host's `IComponentHandler`: how a VST3 edit controller reports what the
//! user does in the plugin window (`beginEdit` / `performEdit` / `endEdit`) and
//! asks for a restart (`restartComponent`).
//!
//! A controller does not talk to its processor: the host must forward every
//! `performEdit` value to `IAudioProcessor::process` as an input parameter
//! change, and that is what [`EditState::drain_edits`] feeds.
//!
//! The callbacks come from the plugin's UI thread (the Win32 thread of its
//! editor) and are lock-free, so a plugin that calls them from elsewhere cannot
//! block the audio thread.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::ffi::{
    FUnknown, IComponentHandlerVtbl, TResult, Tuid, IID_FUNKNOWN, IID_ICOMPONENT_HANDLER,
    K_INVALID_ARGUMENT, K_NO_INTERFACE, K_RESULT_OK,
};
use super::host_context::{static_add_ref, static_release, tuid_eq};
use crate::audio_graph::plugin_params::ValueModulatedParam;
use crate::plugin_formats::clap::ffi::{
    mark_plugin_params_rescan, mark_plugin_restart_requested, mark_plugin_state_edited,
    CLAP_PARAM_RESCAN_INFO, CLAP_PARAM_RESCAN_TEXT,
};
use crate::plugin_system::param_queue::{ParamChange, ParamQueue};

/// `Vst::RestartFlags`.
const RESTART_RELOAD_COMPONENT: i32 = 1 << 0;
const RESTART_IO_CHANGED: i32 = 1 << 1;
const RESTART_PARAM_VALUES_CHANGED: i32 = 1 << 2;
const RESTART_LATENCY_CHANGED: i32 = 1 << 3;
const RESTART_PARAM_TITLES_CHANGED: i32 = 1 << 4;
const RESTART_MIDI_CC_ASSIGNMENT_CHANGED: i32 = 1 << 5;

/// Reactivating the component is the way to take these into account.
const REACTIVATING_RESTARTS: i32 =
    RESTART_RELOAD_COMPONENT | RESTART_IO_CHANGED | RESTART_LATENCY_CHANGED;

const EDIT_QUEUE_CAPACITY: usize = 256;
const MAX_GESTURES: usize = 8;
const GESTURE_OCCUPIED: u64 = 1 << 32;

/// What the plugin window did since the audio thread last looked.
pub struct EditState {
    /// Window thread to audio thread: single producer, single consumer.
    edits: ParamQueue,
    /// Parameters between `beginEdit` and `endEdit`, as `GESTURE_OCCUPIED | id`.
    gestures: [AtomicU64; MAX_GESTURES],
    /// Parameters the host moves by value around a base that follows the user.
    followed: Mutex<Vec<ValueModulatedParam>>,
    reactivation_requested: AtomicBool,
}

impl EditState {
    fn new() -> Self {
        Self {
            edits: ParamQueue::with_capacity(EDIT_QUEUE_CAPACITY),
            gestures: std::array::from_fn(|_| AtomicU64::new(0)),
            followed: Mutex::new(Vec::new()),
            reactivation_requested: AtomicBool::new(false),
        }
    }

    fn begin(&self, native_id: u32) {
        let tag = GESTURE_OCCUPIED | u64::from(native_id);
        if self
            .gestures
            .iter()
            .any(|g| g.load(Ordering::Acquire) == tag)
        {
            return;
        }
        for gesture in &self.gestures {
            if gesture
                .compare_exchange(0, tag, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    fn end(&self, native_id: u32) {
        let tag = GESTURE_OCCUPIED | u64::from(native_id);
        for gesture in &self.gestures {
            let _ = gesture.compare_exchange(tag, 0, Ordering::AcqRel, Ordering::Relaxed);
        }
    }

    fn perform(&self, native_id: u32, normalised: f64) {
        self.edits.push(ParamChange {
            native_id,
            normalised: normalised as f32,
            sample_offset: 0,
        });
        // `try_lock`: a plugin that calls from the audio thread must not wait.
        if let Ok(followed) = self.followed.try_lock() {
            if let Some(param) = followed.iter().find(|param| param.native_id == native_id) {
                param.set_base(normalised as f32);
            }
        }
    }

    fn restart(&self, flags: i32) {
        if flags & REACTIVATING_RESTARTS != 0 {
            self.reactivation_requested.store(true, Ordering::Release);
            mark_plugin_restart_requested();
        }
        // A recompile prepares the instrument again, which reads the new mapping.
        if flags & RESTART_MIDI_CC_ASSIGNMENT_CHANGED != 0 {
            mark_plugin_restart_requested();
        }
        if flags & (RESTART_PARAM_VALUES_CHANGED | RESTART_PARAM_TITLES_CHANGED) != 0 {
            let titles = flags & RESTART_PARAM_TITLES_CHANGED != 0;
            let info = if titles { CLAP_PARAM_RESCAN_INFO } else { 0 };
            mark_plugin_params_rescan(CLAP_PARAM_RESCAN_TEXT | info);
        }
    }

    /// Audio thread: calls `f` for every value the user set since the last block.
    pub fn drain_edits(&self, f: impl FnMut(ParamChange)) {
        self.edits.drain(f);
    }

    /// Audio thread: the user is dragging this control, so modulation pauses.
    pub fn is_editing(&self, native_id: u32) -> bool {
        let tag = GESTURE_OCCUPIED | u64::from(native_id);
        self.gestures
            .iter()
            .any(|g| g.load(Ordering::Acquire) == tag)
    }

    /// Command thread: the parameters modulated by value from now on; returns
    /// the previous ones.
    pub fn replace_followed(&self, now: Vec<ValueModulatedParam>) -> Vec<ValueModulatedParam> {
        let mut followed = self.followed.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::replace(&mut *followed, now)
    }

    /// Command thread: a program or preset moved values behind the host's back, so
    /// the base of each parameter modulated by value is read from the plugin again.
    pub fn rebase_followed(&self, read: impl Fn(u32) -> Option<f64>) {
        let followed = self.followed.lock().unwrap_or_else(|e| e.into_inner());
        for param in followed.iter() {
            if let Some(value) = read(param.native_id) {
                param.set_base(value as f32);
            }
        }
    }

    /// Whether the plugin asked to be reactivated since the last call.
    pub fn take_reactivation_request(&self) -> bool {
        self.reactivation_requested.swap(false, Ordering::AcqRel)
    }

    /// Asks for the reactivation again, e.g. when the plugin was busy.
    pub fn request_reactivation(&self) {
        self.reactivation_requested.store(true, Ordering::Release);
        mark_plugin_restart_requested();
    }
}

/// The COM object handed to the controller.  `vtbl` MUST stay at offset 0.
#[repr(C)]
pub struct ComponentHandler {
    vtbl: *const IComponentHandlerVtbl,
    state: Arc<EditState>,
}

// SAFETY: `vtbl` points to an immutable static and `EditState` is `Sync`.
unsafe impl Send for ComponentHandler {}
unsafe impl Sync for ComponentHandler {}

impl ComponentHandler {
    pub fn boxed() -> Box<Self> {
        Box::new(Self {
            vtbl: &HANDLER_VTBL,
            state: Arc::new(EditState::new()),
        })
    }

    pub fn state(&self) -> &EditState {
        &self.state
    }

    /// Stable for as long as the box lives; the instance keeps it past the controller.
    pub fn as_funknown(&self) -> *mut FUnknown {
        self as *const Self as *mut FUnknown
    }
}

unsafe fn state<'a>(this: *mut FUnknown) -> &'a EditState {
    &(*(this as *const ComponentHandler)).state
}

unsafe extern "C" fn handler_qi(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if obj.is_null() {
        return K_INVALID_ARGUMENT;
    }
    if tuid_eq(iid, &IID_ICOMPONENT_HANDLER) || tuid_eq(iid, &IID_FUNKNOWN) {
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    K_NO_INTERFACE
}

unsafe extern "C" fn handler_begin_edit(this: *mut FUnknown, id: u32) -> TResult {
    state(this).begin(id);
    K_RESULT_OK
}

unsafe extern "C" fn handler_perform_edit(this: *mut FUnknown, id: u32, value: f64) -> TResult {
    state(this).perform(id, value);
    mark_plugin_state_edited();
    K_RESULT_OK
}

unsafe extern "C" fn handler_end_edit(this: *mut FUnknown, id: u32) -> TResult {
    state(this).end(id);
    mark_plugin_state_edited();
    K_RESULT_OK
}

unsafe extern "C" fn handler_restart_component(this: *mut FUnknown, flags: i32) -> TResult {
    state(this).restart(flags);
    K_RESULT_OK
}

static HANDLER_VTBL: IComponentHandlerVtbl = IComponentHandlerVtbl {
    query_interface: handler_qi,
    add_ref: static_add_ref,
    release: static_release,
    begin_edit: handler_begin_edit,
    perform_edit: handler_perform_edit,
    end_edit: handler_end_edit,
    restart_component: handler_restart_component,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_graph::plugin_params::PluginParamRange;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn the_handler_answers_the_sdk_codes_to_a_query() {
        let handler = ComponentHandler::boxed();
        let this = handler.as_funknown();
        let query = |iid: &Tuid, obj: *mut *mut c_void| unsafe {
            ((*(*this).vtbl).query_interface)(this, iid, obj)
        };
        let mut found: *mut c_void = null_mut();
        let unknown = [7u8; 16];

        assert_eq!(query(&IID_ICOMPONENT_HANDLER, &mut found), K_RESULT_OK);
        assert_eq!(found, this as *mut c_void);
        assert_eq!(query(&IID_FUNKNOWN, &mut found), K_RESULT_OK);
        assert_eq!(query(&unknown, &mut found), K_NO_INTERFACE);
        assert!(
            found.is_null(),
            "nothing is handed out for an unknown interface"
        );
        assert_eq!(
            query(&IID_ICOMPONENT_HANDLER, null_mut()),
            K_INVALID_ARGUMENT
        );
    }

    fn call_begin(handler: &ComponentHandler, id: u32) {
        let this = handler.as_funknown();
        unsafe { ((*handler.vtbl).begin_edit)(this, id) };
    }

    fn call_perform(handler: &ComponentHandler, id: u32, value: f64) {
        let this = handler.as_funknown();
        unsafe { ((*handler.vtbl).perform_edit)(this, id, value) };
    }

    fn call_end(handler: &ComponentHandler, id: u32) {
        let this = handler.as_funknown();
        unsafe { ((*handler.vtbl).end_edit)(this, id) };
    }

    fn drained(handler: &ComponentHandler) -> Vec<(u32, f32)> {
        let mut edits = Vec::new();
        handler
            .state()
            .drain_edits(|change| edits.push((change.native_id, change.normalised)));
        edits
    }

    #[test]
    fn edits_reach_the_audio_thread_in_order() {
        let handler = ComponentHandler::boxed();
        call_begin(&handler, 7);
        call_perform(&handler, 7, 0.25);
        call_perform(&handler, 7, 0.5);
        call_end(&handler, 7);
        assert_eq!(drained(&handler), vec![(7, 0.25), (7, 0.5)]);
        assert!(drained(&handler).is_empty());
    }

    #[test]
    fn a_gesture_lasts_from_begin_to_end() {
        let handler = ComponentHandler::boxed();
        assert!(!handler.state().is_editing(3));
        call_begin(&handler, 3);
        call_begin(&handler, 3);
        assert!(handler.state().is_editing(3));
        assert!(!handler.state().is_editing(4));
        call_end(&handler, 3);
        assert!(!handler.state().is_editing(3));
    }

    #[test]
    fn an_edit_moves_the_base_of_a_value_modulated_parameter() {
        let handler = ComponentHandler::boxed();
        let base = Arc::new(AtomicU32::new(0.5f32.to_bits()));
        let param = ValueModulatedParam::new(9, PluginParamRange::UNKNOWN, Arc::clone(&base));
        handler.state().replace_followed(vec![param.clone()]);
        call_perform(&handler, 9, 0.75);
        call_perform(&handler, 10, 0.1);
        assert_eq!(param.base_value(), 0.75);
        assert_eq!(f32::from_bits(base.load(Ordering::Relaxed)), 0.75);
    }

    // The unsaved / rescan / restart flags are process-wide and other unit tests
    // assert on them, so these tests only check the per-handler state.
    #[test]
    fn a_latency_change_asks_for_one_reactivation() {
        let handler = ComponentHandler::boxed();
        let this = handler.as_funknown();
        unsafe { ((*handler.vtbl).restart_component)(this, RESTART_LATENCY_CHANGED) };
        assert!(handler.state().take_reactivation_request());
        assert!(!handler.state().take_reactivation_request());
    }

    #[test]
    fn a_new_midi_mapping_does_not_reactivate_the_plugin() {
        let handler = ComponentHandler::boxed();
        let this = handler.as_funknown();
        unsafe { ((*handler.vtbl).restart_component)(this, RESTART_MIDI_CC_ASSIGNMENT_CHANGED) };
        assert!(!handler.state().take_reactivation_request());
    }
}
