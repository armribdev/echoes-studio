//! Tells a VST3 edit controller about its track (`IInfoListener`).
//!
//! The controller gets an `IAttributeList` with the name and color of the track,
//! the way `clap.track-info` does for CLAP.  Only what the project knows is sent:
//! no index, no UID, no pre/post-fader location, no image.  A track without a
//! color announces none.

use std::ffi::c_void;
use std::ptr::null_mut;

use super::attribute_list::OwnedAttributeList;
use vst3_ffi::{FUnknown, IInfoListenerVtbl, IID_IINFO_LISTENER, K_RESULT_OK};

// `ivstchannelcontextinfo.h`, `namespace ChannelContext`.
pub const CHANNEL_NAME_KEY: &str = "channel name";
pub const CHANNEL_NAME_LENGTH_KEY: &str = "channel name length";
pub const CHANNEL_COLOR_KEY: &str = "channel color";

const OPAQUE: u32 = 0xFF00_0000;

/// The list a controller receives for a track with this `name` and opaque
/// `[red, green, blue]` color. The length counts UTF-16 units, the terminator excluded;
/// the color is an opaque `ColorSpec` (ARGB) read with `getInt`.
pub fn channel_context_list(name: Option<&str>, color: Option<[u8; 3]>) -> OwnedAttributeList {
    let list = OwnedAttributeList::new();
    if let Some(name) = name {
        list.set_text(CHANNEL_NAME_KEY, name);
        list.set_int(CHANNEL_NAME_LENGTH_KEY, name.encode_utf16().count() as i64);
    }
    if let Some([red, green, blue]) = color {
        let color = OPAQUE | (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue);
        list.set_int(CHANNEL_COLOR_KEY, i64::from(color));
    }
    list
}

/// Gives the track to the `IInfoListener` of `object` (an edit controller).  Returns
/// whether the object has one and answered `kResultOk`.
///
/// # Safety
/// `object` must be a live COM object.  Call off the audio thread, under the
/// instance FFI gate.
pub unsafe fn notify_channel_context(
    object: *mut FUnknown,
    name: Option<&str>,
    color: Option<[u8; 3]>,
) -> bool {
    let mut found: *mut c_void = null_mut();
    let result = ((*(*object).vtbl).query_interface)(object, &IID_IINFO_LISTENER, &mut found);
    if result != K_RESULT_OK || found.is_null() {
        return false;
    }
    let listener = found as *mut FUnknown;
    let list = channel_context_list(name, color);
    let vtbl = (*listener).vtbl as *const IInfoListenerVtbl;
    let result = ((*vtbl).set_channel_context_infos)(listener, list.as_funknown());
    ((*(*listener).vtbl).release)(listener);
    result == K_RESULT_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute_list::Value;

    fn text(units: &str) -> Value {
        Value::Text(units.encode_utf16().collect())
    }

    #[test]
    fn a_named_colored_track_announces_both() {
        let list = channel_context_list(Some("Lead"), Some([0x10, 0x20, 0x30]));
        assert_eq!(list.value(CHANNEL_NAME_KEY), Some(text("Lead")));
        assert_eq!(list.value(CHANNEL_NAME_LENGTH_KEY), Some(Value::Int(4)));
        assert_eq!(list.value(CHANNEL_COLOR_KEY), Some(Value::Int(0xFF10_2030)));
    }

    #[test]
    fn a_track_without_a_color_announces_none() {
        let list = channel_context_list(Some("Bass"), None);
        assert_eq!(list.value(CHANNEL_NAME_KEY), Some(text("Bass")));
        assert_eq!(list.value(CHANNEL_COLOR_KEY), None);
    }

    #[test]
    fn a_track_without_a_name_announces_its_color_only() {
        let list = channel_context_list(None, Some([255, 0, 128]));
        assert_eq!(list.value(CHANNEL_NAME_KEY), None);
        assert_eq!(list.value(CHANNEL_NAME_LENGTH_KEY), None);
        assert_eq!(list.value(CHANNEL_COLOR_KEY), Some(Value::Int(0xFFFF_0080)));
    }

    #[test]
    fn a_track_with_neither_announces_nothing() {
        let list = channel_context_list(None, None);
        for key in [CHANNEL_NAME_KEY, CHANNEL_NAME_LENGTH_KEY, CHANNEL_COLOR_KEY] {
            assert_eq!(list.value(key), None, "{key}");
        }
    }

    #[test]
    fn the_name_length_counts_utf16_units() {
        let list = channel_context_list(Some("Piste é 🎹"), None);
        assert_eq!(list.value(CHANNEL_NAME_LENGTH_KEY), Some(Value::Int(10)));
        assert_eq!(list.value(CHANNEL_NAME_KEY), Some(text("Piste é 🎹")));
    }

    #[test]
    fn the_length_is_the_one_of_the_name_given() {
        let long = "é".repeat(127);
        let list = channel_context_list(Some(&long), None);
        let Some(Value::Text(units)) = list.value(CHANNEL_NAME_KEY) else {
            panic!("no name");
        };
        assert_eq!(units.len(), 127);
        assert_eq!(list.value(CHANNEL_NAME_LENGTH_KEY), Some(Value::Int(127)));
    }
}
