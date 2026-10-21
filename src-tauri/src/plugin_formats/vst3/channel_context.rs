//! Tells a VST3 edit controller about its track (`IInfoListener`).
//!
//! The controller gets an `IAttributeList` with the name and color of the track,
//! the way `clap.track-info` does for CLAP.  Only what the project knows is sent:
//! no index, no UID, no pre/post-fader location, no image.  A track without a
//! color announces none.

use std::ffi::c_void;
use std::ptr::null_mut;

use super::attribute_list::OwnedAttributeList;
use super::ffi::{FUnknown, IInfoListenerVtbl, IID_IINFO_LISTENER, K_RESULT_OK};
use crate::plugin_formats::clap::ffi::HostTrackInfo;

// `ivstchannelcontextinfo.h`, `namespace ChannelContext`.
pub(crate) const CHANNEL_NAME_KEY: &str = "channel name";
pub(crate) const CHANNEL_NAME_LENGTH_KEY: &str = "channel name length";
pub(crate) const CHANNEL_COLOR_KEY: &str = "channel color";

const OPAQUE: u32 = 0xFF00_0000;

/// The list a controller receives for `info`.  The length counts UTF-16 units, the
/// terminator excluded; the color is an opaque `ColorSpec` (ARGB) read with `getInt`.
pub(crate) fn channel_context_list(info: &HostTrackInfo) -> OwnedAttributeList {
    let list = OwnedAttributeList::new();
    if let Some(name) = &info.name {
        list.set_text(CHANNEL_NAME_KEY, name);
        list.set_int(CHANNEL_NAME_LENGTH_KEY, name.encode_utf16().count() as i64);
    }
    if let Some([red, green, blue]) = info.color {
        let color = OPAQUE | (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue);
        list.set_int(CHANNEL_COLOR_KEY, i64::from(color));
    }
    list
}

/// Gives `info` to the `IInfoListener` of `object` (an edit controller).  Returns
/// whether the object has one and answered `kResultOk`.
///
/// # Safety
/// `object` must be a live COM object.  Call off the audio thread, under the
/// instance FFI gate.
pub(crate) unsafe fn notify_channel_context(object: *mut FUnknown, info: &HostTrackInfo) -> bool {
    let mut found: *mut c_void = null_mut();
    let result = ((*(*object).vtbl).query_interface)(object, &IID_IINFO_LISTENER, &mut found);
    if result != K_RESULT_OK || found.is_null() {
        return false;
    }
    let listener = found as *mut FUnknown;
    let list = channel_context_list(info);
    let vtbl = (*listener).vtbl as *const IInfoListenerVtbl;
    let result = ((*vtbl).set_channel_context_infos)(listener, list.as_funknown());
    ((*(*listener).vtbl).release)(listener);
    result == K_RESULT_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_formats::vst3::attribute_list::Value;
    use crate::plugin_formats::vst3::fake_plugin::{FakeListener, Received};

    fn text(units: &str) -> Value {
        Value::Text(units.encode_utf16().collect())
    }

    #[test]
    fn a_named_colored_track_announces_both() {
        let list = channel_context_list(&HostTrackInfo::new("Lead", Some([0x10, 0x20, 0x30])));
        assert_eq!(list.value(CHANNEL_NAME_KEY), Some(text("Lead")));
        assert_eq!(list.value(CHANNEL_NAME_LENGTH_KEY), Some(Value::Int(4)));
        assert_eq!(list.value(CHANNEL_COLOR_KEY), Some(Value::Int(0xFF10_2030)));
    }

    #[test]
    fn a_track_without_a_color_announces_none() {
        let list = channel_context_list(&HostTrackInfo::new("Bass", None));
        assert_eq!(list.value(CHANNEL_NAME_KEY), Some(text("Bass")));
        assert_eq!(list.value(CHANNEL_COLOR_KEY), None);
    }

    #[test]
    fn a_track_without_a_name_announces_its_color_only() {
        let list = channel_context_list(&HostTrackInfo::new("", Some([255, 0, 128])));
        assert_eq!(list.value(CHANNEL_NAME_KEY), None);
        assert_eq!(list.value(CHANNEL_NAME_LENGTH_KEY), None);
        assert_eq!(list.value(CHANNEL_COLOR_KEY), Some(Value::Int(0xFFFF_0080)));
    }

    #[test]
    fn a_track_with_neither_announces_nothing() {
        let list = channel_context_list(&HostTrackInfo::new("", None));
        for key in [CHANNEL_NAME_KEY, CHANNEL_NAME_LENGTH_KEY, CHANNEL_COLOR_KEY] {
            assert_eq!(list.value(key), None, "{key}");
        }
    }

    #[test]
    fn the_name_length_counts_utf16_units() {
        let list = channel_context_list(&HostTrackInfo::new("Piste é 🎹", None));
        assert_eq!(list.value(CHANNEL_NAME_LENGTH_KEY), Some(Value::Int(10)));
        assert_eq!(list.value(CHANNEL_NAME_KEY), Some(text("Piste é 🎹")));
    }

    #[test]
    fn a_very_long_name_is_the_cut_one() {
        let long = "é".repeat(300);
        let list = channel_context_list(&HostTrackInfo::new(&long, None));
        let Some(Value::Text(units)) = list.value(CHANNEL_NAME_KEY) else {
            panic!("no name");
        };
        assert_eq!(units.len(), 127, "255 bytes of two-byte characters");
        assert_eq!(list.value(CHANNEL_NAME_LENGTH_KEY), Some(Value::Int(127)));
    }

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
}
