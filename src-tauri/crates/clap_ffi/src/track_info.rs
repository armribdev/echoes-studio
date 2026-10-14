//! Host side of `clap.track-info`: the name and color of the track a plugin sits on.
//!
//! The plugin asks through `clap_host_track_info.get`, which the spec marks
//! `[main-thread]` but a plugin may reach from any thread, so the info lives in
//! a [`TrackInfoCell`] that is read without locks and without allocating.  The
//! host writes it from the command thread (or the bridge's main thread) and
//! then calls the plugin's `changed`.
//!
//! Only what the host really knows is reported: a name and a color.  Channel
//! count, port type and the return / bus / master flags stay unset.

use std::ffi::c_char;
use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};

use crate::{host_context, ClapHost, ClapPlugin, CLAP_NAME_SIZE};

pub const CLAP_EXT_TRACK_INFO: &[u8] = b"clap.track-info/1\0";
/// Draft id some plugins still use; the spec calls it fully compatible.
pub const CLAP_EXT_TRACK_INFO_COMPAT: &[u8] = b"clap.track-info.draft/1\0";

pub const CLAP_TRACK_INFO_HAS_TRACK_NAME: u64 = 1 << 0;
pub const CLAP_TRACK_INFO_HAS_TRACK_COLOR: u64 = 1 << 1;
pub const CLAP_TRACK_INFO_HAS_AUDIO_CHANNEL: u64 = 1 << 2;
pub const CLAP_TRACK_INFO_IS_FOR_RETURN_TRACK: u64 = 1 << 3;
pub const CLAP_TRACK_INFO_IS_FOR_BUS: u64 = 1 << 4;
pub const CLAP_TRACK_INFO_IS_FOR_MASTER: u64 = 1 << 5;

/// `clap_color`: field order is the ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClapColor {
    pub alpha: u8,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

#[repr(C)]
pub struct ClapTrackInfo {
    pub flags: u64,
    pub name: [u8; CLAP_NAME_SIZE],
    pub color: ClapColor,
    pub audio_channel_count: i32,
    pub audio_port_type: *const c_char,
}

/// `clap_plugin_track_info`: the plugin side, `[main-thread]`.
#[repr(C)]
pub struct ClapPluginTrackInfo {
    pub changed: unsafe extern "C" fn(*const ClapPlugin),
}

/// `clap_host_track_info`: the host side.
#[repr(C)]
pub struct ClapHostTrackInfo {
    pub get: unsafe extern "C" fn(*const ClapHost, *mut ClapTrackInfo) -> bool,
}

pub(crate) static HOST_TRACK_INFO: ClapHostTrackInfo = ClapHostTrackInfo {
    get: host_track_info_get,
};

/// A name leaves room for the NUL of the C array.
const MAX_NAME_BYTES: usize = CLAP_NAME_SIZE - 1;

/// What the host tells a plugin about its track.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostTrackInfo {
    pub name: Option<String>,
    /// Opaque `[red, green, blue]`; `None` when the track has no color of its own.
    pub color: Option<[u8; 3]>,
}

impl HostTrackInfo {
    /// An empty name counts as none, and a long one is cut at a character boundary.
    pub fn new(name: &str, color: Option<[u8; 3]>) -> Self {
        let mut end = name.len().min(MAX_NAME_BYTES);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            name: (end > 0).then(|| name[..end].to_owned()),
            color,
        }
    }

    /// Wire form for the bridge: `[present:u8 (1 name, 2 color), r, g, b, name…]`.
    pub fn encode(&self) -> Vec<u8> {
        let present = u8::from(self.name.is_some()) | (u8::from(self.color.is_some()) << 1);
        let [red, green, blue] = self.color.unwrap_or_default();
        let mut bytes = vec![present, red, green, blue];
        bytes.extend_from_slice(self.name.as_deref().unwrap_or_default().as_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (head, name) = bytes.split_first_chunk::<4>()?;
        let [present, red, green, blue] = *head;
        let name = std::str::from_utf8(name).ok()?;
        Some(Self::new(
            if present & 1 != 0 { name } else { "" },
            (present & 2 != 0).then_some([red, green, blue]),
        ))
    }

    /// [`Self::encode`] as hexadecimal text, for a command-line argument.
    pub fn to_hex(&self) -> String {
        self.encode()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        let bytes: Option<Vec<u8>> = text
            .as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect();
        Self::decode(&bytes?)
    }
}

const NAME_WORDS: usize = CLAP_NAME_SIZE / 8;
const PRESENT_KNOWN: u32 = 1 << 0;
const PRESENT_NAME: u32 = 1 << 1;
const PRESENT_COLOR: u32 = 1 << 2;
/// Reads that hit a write in progress this many times give up.
const READ_ATTEMPTS: usize = 8;

/// The track info of one plugin, readable from any thread without a lock.
///
/// A sequence lock: a write makes `sequence` odd while it runs, and a read that
/// saw it odd or changed starts again.  Writers exclude each other (they are
/// off-RT threads); a reader never waits on one — after [`READ_ATTEMPTS`] it
/// reports no info instead.
pub(crate) struct TrackInfoCell {
    sequence: AtomicU32,
    present: AtomicU32,
    color: AtomicU32,
    name: [AtomicU64; NAME_WORDS],
}

impl TrackInfoCell {
    pub(crate) fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            present: AtomicU32::new(0),
            color: AtomicU32::new(0),
            name: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// Stores `info`; returns whether it differs from what was there, the first
    /// store counting as a difference.
    pub(crate) fn store(&self, info: &HostTrackInfo) -> bool {
        let before = self.lock_for_write();
        let (present, color, name) = pack(info);
        let changed = present != self.present.load(Ordering::Relaxed)
            || color != self.color.load(Ordering::Relaxed)
            || name
                .iter()
                .zip(&self.name)
                .any(|(word, stored)| *word != stored.load(Ordering::Relaxed));
        if changed {
            self.present.store(present, Ordering::Relaxed);
            self.color.store(color, Ordering::Relaxed);
            for (word, stored) in name.iter().zip(&self.name) {
                stored.store(*word, Ordering::Relaxed);
            }
        }
        self.sequence
            .store(before.wrapping_add(2), Ordering::Release);
        changed
    }

    /// Takes the write side (sequence odd) and returns the sequence before it.
    fn lock_for_write(&self) -> u32 {
        loop {
            let sequence = self.sequence.load(Ordering::Relaxed);
            if sequence & 1 == 0
                && self
                    .sequence
                    .compare_exchange_weak(
                        sequence,
                        sequence.wrapping_add(1),
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
            {
                fence(Ordering::Release);
                return sequence;
            }
            std::hint::spin_loop();
        }
    }

    /// Fills `out` and returns true when the host knows the track.  Only the
    /// fields the host knows are set; the others stay zero.
    pub(crate) fn read_into(&self, out: &mut ClapTrackInfo) -> bool {
        for _ in 0..READ_ATTEMPTS {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 == 0 {
                let present = self.present.load(Ordering::Relaxed);
                let color = self.color.load(Ordering::Relaxed);
                for (chunk, word) in out.name.chunks_exact_mut(8).zip(&self.name) {
                    chunk.copy_from_slice(&word.load(Ordering::Relaxed).to_le_bytes());
                }
                fence(Ordering::Acquire);
                if self.sequence.load(Ordering::Relaxed) == before {
                    return fill(out, present, color);
                }
            }
            std::hint::spin_loop();
        }
        out.flags = 0;
        false
    }
}

fn pack(info: &HostTrackInfo) -> (u32, u32, [u64; NAME_WORDS]) {
    let mut present = PRESENT_KNOWN;
    let mut bytes = [0u8; CLAP_NAME_SIZE];
    if let Some(name) = &info.name {
        present |= PRESENT_NAME;
        let length = name.len().min(MAX_NAME_BYTES);
        bytes[..length].copy_from_slice(&name.as_bytes()[..length]);
    }
    let color = info.color.map_or(0, |[red, green, blue]| {
        present |= PRESENT_COLOR;
        u32::from_be_bytes([0, red, green, blue])
    });
    let mut words = [0u64; NAME_WORDS];
    for (word, chunk) in words.iter_mut().zip(bytes.chunks_exact(8)) {
        *word = u64::from_le_bytes(chunk.try_into().unwrap_or_default());
    }
    (present, color, words)
}

fn fill(out: &mut ClapTrackInfo, present: u32, color: u32) -> bool {
    out.flags = 0;
    out.color = ClapColor::default();
    out.audio_channel_count = 0;
    out.audio_port_type = std::ptr::null();
    if present & PRESENT_KNOWN == 0 {
        out.name = [0; CLAP_NAME_SIZE];
        return false;
    }
    if present & PRESENT_NAME != 0 {
        out.flags |= CLAP_TRACK_INFO_HAS_TRACK_NAME;
    } else {
        out.name = [0; CLAP_NAME_SIZE];
    }
    if present & PRESENT_COLOR != 0 {
        out.flags |= CLAP_TRACK_INFO_HAS_TRACK_COLOR;
        let [_, red, green, blue] = color.to_be_bytes();
        out.color = ClapColor {
            alpha: 255,
            red,
            green,
            blue,
        };
    }
    true
}

unsafe extern "C" fn host_track_info_get(host: *const ClapHost, info: *mut ClapTrackInfo) -> bool {
    let (Some(context), Some(info)) = (host_context(host), info.as_mut()) else {
        return false;
    };
    context.read_track_info(info)
}

/// Calls the plugin's `clap_plugin_track_info.changed`, under the id it
/// provides.  False when the plugin has no such extension.
///
/// # Safety
/// `plugin` must be live, and the call must run where CLAP allows main-thread
/// calls on it.
pub unsafe fn notify_track_info_changed(plugin: *const ClapPlugin) -> bool {
    for id in [CLAP_EXT_TRACK_INFO, CLAP_EXT_TRACK_INFO_COMPAT] {
        let extension = ((*plugin).get_extension)(plugin, id.as_ptr() as *const c_char)
            as *const ClapPluginTrackInfo;
        if let Some(extension) = extension.as_ref() {
            (extension.changed)(plugin);
            return true;
        }
    }
    false
}

/// What a plugin sees of the host table, for the tests of this module and of the
/// engine: the extension answered under `id`.
pub unsafe fn host_track_info_extension(
    host: *const ClapHost,
    id: &[u8],
) -> Option<&'static ClapHostTrackInfo> {
    let extension = ((*host).get_extension)(host, id.as_ptr() as *const c_char);
    (extension as *const ClapHostTrackInfo).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{make_host, ClapProcess, ClapProcessStatus};
    use std::mem::{offset_of, size_of};

    fn blank() -> ClapTrackInfo {
        // SAFETY: plain data; an all-zero pattern is valid for every field.
        unsafe { std::mem::zeroed() }
    }

    fn read(host: &ClapHost) -> Option<ClapTrackInfo> {
        let mut info = blank();
        info.name = [0xAA; CLAP_NAME_SIZE];
        let extension = unsafe { host_track_info_extension(host, CLAP_EXT_TRACK_INFO) }.unwrap();
        unsafe { (extension.get)(host, &mut info) }.then_some(info)
    }

    fn name_of(info: &ClapTrackInfo) -> &str {
        let length = info.name.iter().position(|&byte| byte == 0).unwrap();
        std::str::from_utf8(&info.name[..length]).unwrap()
    }

    #[test]
    fn the_struct_layout_follows_the_spec() {
        assert_eq!(size_of::<ClapColor>(), 4);
        assert_eq!(offset_of!(ClapTrackInfo, flags), 0);
        assert_eq!(offset_of!(ClapTrackInfo, name), 8);
        assert_eq!(offset_of!(ClapTrackInfo, color), 8 + CLAP_NAME_SIZE);
        assert_eq!(
            offset_of!(ClapTrackInfo, audio_channel_count),
            8 + CLAP_NAME_SIZE + 4
        );
        assert_eq!(size_of::<ClapTrackInfo>() % 8, 0);
        assert_eq!(offset_of!(ClapColor, alpha) + 1, offset_of!(ClapColor, red));
        assert_eq!(
            offset_of!(ClapColor, green) + 1,
            offset_of!(ClapColor, blue)
        );
    }

    #[test]
    fn the_host_answers_the_extension_under_both_ids() {
        let host = make_host();
        for id in [CLAP_EXT_TRACK_INFO, CLAP_EXT_TRACK_INFO_COMPAT] {
            assert!(unsafe { host_track_info_extension(&*host, id) }.is_some());
        }
    }

    #[test]
    fn a_track_the_host_never_described_is_not_reported() {
        let host = make_host();
        assert!(read(&host).is_none());
    }

    #[test]
    fn name_and_color_are_reported_and_nothing_else() {
        let host = make_host();
        host.context()
            .unwrap()
            .set_track_info(&HostTrackInfo::new("Lead", Some([0x12, 0x34, 0x56])));
        let info = read(&host).unwrap();

        assert_eq!(
            info.flags,
            CLAP_TRACK_INFO_HAS_TRACK_NAME | CLAP_TRACK_INFO_HAS_TRACK_COLOR
        );
        assert_eq!(name_of(&info), "Lead");
        assert_eq!(
            info.color,
            ClapColor {
                alpha: 255,
                red: 0x12,
                green: 0x34,
                blue: 0x56
            }
        );
        assert_eq!(info.audio_channel_count, 0);
        assert!(info.audio_port_type.is_null());
    }

    #[test]
    fn a_track_without_color_leaves_the_color_flag_unset() {
        let host = make_host();
        host.context()
            .unwrap()
            .set_track_info(&HostTrackInfo::new("Bass", None));
        let info = read(&host).unwrap();

        assert_eq!(info.flags, CLAP_TRACK_INFO_HAS_TRACK_NAME);
        assert_eq!(info.color, ClapColor::default());
    }

    #[test]
    fn a_track_without_name_or_color_is_known_with_no_flag() {
        let host = make_host();
        host.context()
            .unwrap()
            .set_track_info(&HostTrackInfo::new("", None));
        let info = read(&host).unwrap();

        assert_eq!(info.flags, 0);
        assert!(info.name.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn storing_the_same_info_again_is_not_a_change() {
        let cell = TrackInfoCell::new();
        let info = HostTrackInfo::new("Lead", Some([1, 2, 3]));
        assert!(cell.store(&info));
        assert!(!cell.store(&info));
        assert!(cell.store(&HostTrackInfo::new("Lead", Some([1, 2, 4]))));
        assert!(cell.store(&HostTrackInfo::new("Lead 2", Some([1, 2, 4]))));
        assert!(cell.store(&HostTrackInfo::new("Lead 2", None)));
    }

    #[test]
    fn a_shorter_name_leaves_no_trace_of_the_longer_one() {
        let host = make_host();
        let context = host.context().unwrap();
        context.set_track_info(&HostTrackInfo::new("A long track name", None));
        context.set_track_info(&HostTrackInfo::new("Hi", None));

        assert_eq!(name_of(&read(&host).unwrap()), "Hi");
    }

    #[test]
    fn a_long_name_is_cut_on_a_character_boundary() {
        let long = "é".repeat(200);
        let info = HostTrackInfo::new(&long, None);
        let name = info.name.unwrap();

        assert!(name.len() < CLAP_NAME_SIZE);
        assert_eq!(name, "é".repeat(127));
    }

    #[test]
    fn the_wire_form_round_trips() {
        for info in [
            HostTrackInfo::new("Piste é", Some([9, 8, 7])),
            HostTrackInfo::new("Piste", None),
            HostTrackInfo::new("", Some([0, 0, 255])),
            HostTrackInfo::new("", None),
        ] {
            assert_eq!(HostTrackInfo::decode(&info.encode()), Some(info.clone()));
            assert_eq!(HostTrackInfo::from_hex(&info.to_hex()), Some(info));
        }
        assert_eq!(HostTrackInfo::decode(&[1, 0]), None);
        assert_eq!(HostTrackInfo::from_hex("zz"), None);
    }

    static DRAFT_ONLY_CHANGES: AtomicU32 = AtomicU32::new(0);
    static DRAFT_ONLY: ClapPluginTrackInfo = ClapPluginTrackInfo {
        changed: count_draft_only_change,
    };

    unsafe extern "C" fn count_draft_only_change(_: *const ClapPlugin) {
        DRAFT_ONLY_CHANGES.fetch_add(1, Ordering::Relaxed);
    }

    unsafe extern "C" fn answer_to_the_draft_id_only(
        _: *const ClapPlugin,
        id: *const c_char,
    ) -> *const std::ffi::c_void {
        if std::ffi::CStr::from_ptr(id).to_bytes_with_nul() == CLAP_EXT_TRACK_INFO_COMPAT {
            &DRAFT_ONLY as *const ClapPluginTrackInfo as *const std::ffi::c_void
        } else {
            std::ptr::null()
        }
    }

    unsafe extern "C" fn no_extension(
        _: *const ClapPlugin,
        _: *const c_char,
    ) -> *const std::ffi::c_void {
        std::ptr::null()
    }

    unsafe extern "C" fn init_or_start(_: *const ClapPlugin) -> bool {
        true
    }

    unsafe extern "C" fn nothing(_: *const ClapPlugin) {}

    unsafe extern "C" fn activate(_: *const ClapPlugin, _: f64, _: u32, _: u32) -> bool {
        true
    }

    unsafe extern "C" fn process(_: *const ClapPlugin, _: *const ClapProcess) -> ClapProcessStatus {
        0
    }

    fn plugin_with(
        get_extension: unsafe extern "C" fn(
            *const ClapPlugin,
            *const c_char,
        ) -> *const std::ffi::c_void,
    ) -> ClapPlugin {
        ClapPlugin {
            desc: std::ptr::null(),
            plugin_data: std::ptr::null_mut(),
            init: init_or_start,
            destroy: nothing,
            activate,
            deactivate: nothing,
            start_processing: init_or_start,
            stop_processing: nothing,
            reset: nothing,
            process,
            get_extension,
            on_main_thread: nothing,
        }
    }

    #[test]
    fn the_plugin_is_told_under_the_id_it_provides() {
        let draft_only = plugin_with(answer_to_the_draft_id_only);
        assert!(unsafe { notify_track_info_changed(&draft_only) });
        assert_eq!(DRAFT_ONLY_CHANGES.load(Ordering::Relaxed), 1);

        let without = plugin_with(no_extension);
        assert!(!unsafe { notify_track_info_changed(&without) });
        assert_eq!(DRAFT_ONLY_CHANGES.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_reader_never_sees_a_half_written_name() {
        let host = make_host();
        let context = host.context().unwrap();
        let first = HostTrackInfo::new(&"a".repeat(200), Some([1, 1, 1]));
        let second = HostTrackInfo::new(&"b".repeat(200), Some([2, 2, 2]));
        context.set_track_info(&first);

        let host_address = &*host as *const ClapHost as usize;
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let host = host_address as *const ClapHost;
                let extension =
                    unsafe { host_track_info_extension(host, CLAP_EXT_TRACK_INFO) }.unwrap();
                let mut seen = 0;
                for _ in 0..200_000 {
                    let mut info = blank();
                    if !unsafe { (extension.get)(host, &mut info) } {
                        continue;
                    }
                    seen += 1;
                    let name = name_of(&info);
                    assert!(
                        name == "a".repeat(200) || name == "b".repeat(200),
                        "torn name: {name}"
                    );
                    assert_eq!(
                        name.starts_with('a'),
                        info.color.red == 1,
                        "name and color from different writes"
                    );
                }
                seen
            });
            for _ in 0..20_000 {
                context.set_track_info(&second);
                context.set_track_info(&first);
            }
            assert!(reader.join().unwrap() > 0);
        });
    }
}
