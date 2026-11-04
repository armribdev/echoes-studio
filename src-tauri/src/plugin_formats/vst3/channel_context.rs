//! Tells a VST3 edit controller about its track (`IInfoListener`).
//!
//! The objects are in `vst3_host::channel_context`; this adapter hands them the name and
//! color of the engine's [`HostTrackInfo`], the same value `clap.track-info` gets.

use crate::plugin_formats::clap::ffi::HostTrackInfo;
use vst3_ffi::FUnknown;

#[cfg(test)]
pub(crate) use vst3_host::channel_context::{
    CHANNEL_COLOR_KEY, CHANNEL_NAME_KEY, CHANNEL_NAME_LENGTH_KEY,
};

/// Gives `info` to the `IInfoListener` of `object` (an edit controller).  Returns
/// whether the object has one and answered `kResultOk`.
///
/// # Safety
/// `object` must be a live COM object.  Call off the audio thread, under the
/// instance FFI gate.
pub(crate) unsafe fn notify_channel_context(object: *mut FUnknown, info: &HostTrackInfo) -> bool {
    vst3_host::channel_context::notify_channel_context(object, info.name.as_deref(), info.color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_formats::vst3::fake_plugin::{FakeListener, Received};

    #[test]
    fn a_listening_controller_receives_the_list() {
        let mut controller = FakeListener::new(true);
        let info = HostTrackInfo::new("Lead", Some([1, 2, 3]));
        assert!(unsafe { notify_channel_context(controller.as_funknown(), &info) });
        assert_eq!(
            *controller.received.borrow(),
            [Received {
                name: Some("Lead".into()),
                name_length: Some(4),
                color: Some(0xFF01_0203),
            }]
        );
        assert_eq!(
            controller.references.get(),
            1,
            "the queried reference is given back"
        );
    }

    #[test]
    fn a_controller_without_the_interface_is_left_alone() {
        let mut controller = FakeListener::new(false);
        let info = HostTrackInfo::new("Lead", None);
        assert!(!unsafe { notify_channel_context(controller.as_funknown(), &info) });
        assert!(controller.received.borrow().is_empty());
        assert_eq!(controller.references.get(), 1);
    }

    #[test]
    fn a_very_long_name_is_cut_before_it_reaches_the_controller() {
        let mut controller = FakeListener::new(true);
        let info = HostTrackInfo::new(&"é".repeat(300), None);
        assert!(unsafe { notify_channel_context(controller.as_funknown(), &info) });
        assert_eq!(
            controller.received.borrow()[0].name_length,
            Some(127),
            "255 bytes of two-byte characters"
        );
    }
}
