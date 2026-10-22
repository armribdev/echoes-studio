//! Reader of Steinberg `.vstpreset` files (ADR-031, VST3 presets).
//!
//! The file comes from the user's disk, so nothing in it is trusted: every size
//! and offset is checked against the real file length before it is used, and an
//! invalid file is an error, never a panic or a large allocation.
//!
//! Layout (`vstpresetfile.h`, little-endian): a 48-byte header (`VST3`, version
//! `i32`, 32 ASCII hex digits of the class id, `i64` offset of the chunk list),
//! the chunks, then at the offset: `List`, entry count `i32`, and per entry the
//! chunk id (4 bytes), its offset `i64` and its size `i64`.

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const SIGNATURE: &[u8; 4] = b"VST3";
const LIST_ID: &[u8; 4] = b"List";
const COMPONENT_STATE_ID: &[u8; 4] = b"Comp";
const CONTROLLER_STATE_ID: &[u8; 4] = b"Cont";

const CLASS_ID_LENGTH: usize = 32;
const HEADER_SIZE: u64 = 4 + 4 + CLASS_ID_LENGTH as u64 + 8;
const LIST_HEADER_SIZE: u64 = 4 + 4;
const ENTRY_SIZE: u64 = 4 + 8 + 8;
/// The SDK reads at most this many entries.
const MAX_ENTRIES: usize = 128;

/// Refuses larger files before reading any of them.  A chunk is never larger than
/// its file, so this also bounds every allocation.
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum PresetFileError {
    Unreadable(String),
    TooLarge,
    NotAPresetFile,
    Truncated,
    Corrupt(&'static str),
    OtherPlugin,
    NoComponentState,
}

impl fmt::Display for PresetFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(reason) => write!(f, "the preset file cannot be read: {reason}"),
            Self::TooLarge => f.write_str("the preset file is too large"),
            Self::NotAPresetFile => f.write_str("this is not a VST3 preset file"),
            Self::Truncated => f.write_str("the preset file is truncated"),
            Self::Corrupt(reason) => write!(f, "the preset file is corrupt: {reason}"),
            Self::OtherPlugin => f.write_str("this preset belongs to another plugin"),
            Self::NoComponentState => f.write_str("the preset file holds no plugin state"),
        }
    }
}

impl From<io::Error> for PresetFileError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::UnexpectedEof => Self::Truncated,
            _ => Self::Unreadable(error.to_string()),
        }
    }
}

/// What a preset restores: the component's state, and the controller's own state
/// when the file has one.
#[derive(Debug, PartialEq, Eq)]
pub struct PresetChunks {
    pub component: Vec<u8>,
    pub controller: Option<Vec<u8>>,
}

/// The class id of a plugin as a preset file writes it (`FUID::toString`): 32
/// uppercase hex digits in the order of the SDK's `l1 l2 l3 l4` words.  On Windows
/// the id bytes are in COM order, so the first three groups are swapped back.
pub fn class_id_text(class_id: &[u8; 16]) -> String {
    let mut sdk_order = *class_id;
    if cfg!(windows) {
        sdk_order[..4].reverse();
        sdk_order[4..6].reverse();
        sdk_order[6..8].reverse();
    }
    sdk_order.iter().map(|byte| format!("{byte:02X}")).collect()
}

/// Whether the file is a preset for the plugin with this `class_id_text`.  Reads
/// the 48-byte header only, so listing thousands of files stays cheap.
pub fn is_preset_for(path: &Path, class_id: &str) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    matches!(read_header(&mut file), Ok(header) if header.class_id.eq_ignore_ascii_case(class_id))
}

/// Reads the component and controller states of a preset file, checking that it
/// belongs to the plugin with this `class_id_text`.
pub fn read_preset_chunks(path: &Path, class_id: &str) -> Result<PresetChunks, PresetFileError> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    read_chunks(&mut file, length, class_id)
}

struct Header {
    class_id: String,
    list_offset: u64,
}

struct Entry {
    id: [u8; 4],
    offset: u64,
    size: u64,
}

fn read_header(reader: &mut impl Read) -> Result<Header, PresetFileError> {
    let mut bytes = [0u8; HEADER_SIZE as usize];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| match error.kind() {
            io::ErrorKind::UnexpectedEof => PresetFileError::NotAPresetFile,
            _ => error.into(),
        })?;
    if &bytes[..4] != SIGNATURE {
        return Err(PresetFileError::NotAPresetFile);
    }
    // The SDK reads the version at bytes 4..8 and ignores it.
    let class_id = &bytes[8..8 + CLASS_ID_LENGTH];
    if !class_id.iter().all(u8::is_ascii_hexdigit) {
        return Err(PresetFileError::Corrupt("the class id is not hexadecimal"));
    }
    let list_offset = i64::from_le_bytes(bytes[40..48].try_into().expect("8 bytes"));
    Ok(Header {
        class_id: String::from_utf8_lossy(class_id).to_ascii_uppercase(),
        list_offset: u64::try_from(list_offset)
            .map_err(|_| PresetFileError::Corrupt("the chunk list offset is negative"))?,
    })
}

fn read_chunks<R: Read + Seek>(
    reader: &mut R,
    length: u64,
    class_id: &str,
) -> Result<PresetChunks, PresetFileError> {
    if length > MAX_FILE_BYTES {
        return Err(PresetFileError::TooLarge);
    }
    let header = read_header(reader)?;
    if !header.class_id.eq_ignore_ascii_case(class_id) {
        return Err(PresetFileError::OtherPlugin);
    }
    let entries = read_entries(reader, length, header.list_offset)?;
    let find = |id: &[u8; 4]| entries.iter().find(|entry| &entry.id == id);
    let component = find(COMPONENT_STATE_ID).ok_or(PresetFileError::NoComponentState)?;
    Ok(PresetChunks {
        component: read_chunk(reader, component)?,
        controller: find(CONTROLLER_STATE_ID)
            .map(|entry| read_chunk(reader, entry))
            .transpose()?,
    })
}

fn read_entries<R: Read + Seek>(
    reader: &mut R,
    length: u64,
    list_offset: u64,
) -> Result<Vec<Entry>, PresetFileError> {
    if list_offset < HEADER_SIZE {
        return Err(PresetFileError::Corrupt(
            "the chunk list overlaps the header",
        ));
    }
    if list_offset
        .checked_add(LIST_HEADER_SIZE)
        .map_or(true, |end| end > length)
    {
        return Err(PresetFileError::Truncated);
    }
    reader.seek(SeekFrom::Start(list_offset))?;
    let mut list_header = [0u8; LIST_HEADER_SIZE as usize];
    reader.read_exact(&mut list_header)?;
    if &list_header[..4] != LIST_ID {
        return Err(PresetFileError::Corrupt("the chunk list is missing"));
    }
    let count = i32::from_le_bytes(list_header[4..].try_into().expect("4 bytes"));
    let count = usize::try_from(count)
        .map_err(|_| PresetFileError::Corrupt("the chunk count is negative"))?
        .min(MAX_ENTRIES);
    if count == 0 {
        return Err(PresetFileError::Corrupt("the chunk list is empty"));
    }
    let entries_end = list_offset + LIST_HEADER_SIZE + ENTRY_SIZE * count as u64;
    if entries_end > length {
        return Err(PresetFileError::Truncated);
    }
    (0..count).map(|_| read_entry(reader, length)).collect()
}

fn read_entry(reader: &mut impl Read, length: u64) -> Result<Entry, PresetFileError> {
    let mut bytes = [0u8; ENTRY_SIZE as usize];
    reader.read_exact(&mut bytes)?;
    let offset = i64::from_le_bytes(bytes[4..12].try_into().expect("8 bytes"));
    let size = i64::from_le_bytes(bytes[12..20].try_into().expect("8 bytes"));
    let (Ok(offset), Ok(size)) = (u64::try_from(offset), u64::try_from(size)) else {
        return Err(PresetFileError::Corrupt(
            "a chunk has a negative offset or size",
        ));
    };
    if offset < HEADER_SIZE || offset.checked_add(size).map_or(true, |end| end > length) {
        return Err(PresetFileError::Corrupt("a chunk lies outside the file"));
    }
    Ok(Entry {
        id: bytes[..4].try_into().expect("4 bytes"),
        offset,
        size,
    })
}

fn read_chunk<R: Read + Seek>(reader: &mut R, entry: &Entry) -> Result<Vec<u8>, PresetFileError> {
    reader.seek(SeekFrom::Start(entry.offset))?;
    let mut bytes = Vec::new();
    reader.by_ref().take(entry.size).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != entry.size {
        return Err(PresetFileError::Truncated);
    }
    Ok(bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::plugin_formats::vst3::ffi::IID_ICOMPONENT;
    use std::io::Cursor;

    const CLASS: &str = "E831FF31F2D54301928EBBEE25697802";

    /// A chunk as the test writes it: its id and bytes.
    pub(crate) type Chunk<'a> = (&'a [u8; 4], &'a [u8]);

    /// A well-formed file in the SDK's layout: header, chunks, then the list.
    pub(crate) fn preset_file(class_id: &str, chunks: &[Chunk]) -> Vec<u8> {
        let mut file = Vec::new();
        file.extend_from_slice(SIGNATURE);
        file.extend_from_slice(&1i32.to_le_bytes());
        file.extend_from_slice(class_id.as_bytes());
        file.extend_from_slice(&0i64.to_le_bytes());
        let mut entries = Vec::new();
        for (id, bytes) in chunks {
            entries.push((**id, file.len() as i64, bytes.len() as i64));
            file.extend_from_slice(bytes);
        }
        let list_offset = file.len() as i64;
        file[40..48].copy_from_slice(&list_offset.to_le_bytes());
        file.extend_from_slice(LIST_ID);
        file.extend_from_slice(&(entries.len() as i32).to_le_bytes());
        for (id, offset, size) in entries {
            file.extend_from_slice(&id);
            file.extend_from_slice(&offset.to_le_bytes());
            file.extend_from_slice(&size.to_le_bytes());
        }
        file
    }

    fn read(file: &[u8]) -> Result<PresetChunks, PresetFileError> {
        read_chunks(&mut Cursor::new(file), file.len() as u64, CLASS)
    }

    /// Overwrites `bytes` at `at`, to damage a well-formed file.
    fn patched(mut file: Vec<u8>, at: usize, bytes: &[u8]) -> Vec<u8> {
        file[at..at + bytes.len()].copy_from_slice(bytes);
        file
    }

    fn valid() -> Vec<u8> {
        preset_file(CLASS, &[(b"Comp", b"component"), (b"Cont", b"controller")])
    }

    fn list_offset(file: &[u8]) -> usize {
        i64::from_le_bytes(file[40..48].try_into().unwrap()) as usize
    }

    #[test]
    fn reads_the_component_and_the_controller_state() {
        let chunks = read(&valid()).unwrap();
        assert_eq!(chunks.component, b"component");
        assert_eq!(chunks.controller.as_deref(), Some(&b"controller"[..]));
    }

    #[test]
    fn the_controller_state_is_optional() {
        let file = preset_file(CLASS, &[(b"Comp", b"component")]);
        assert_eq!(read(&file).unwrap().controller, None);
    }

    #[test]
    fn other_chunks_are_ignored_and_the_first_of_a_kind_wins() {
        let file = preset_file(
            CLASS,
            &[
                (b"Info", b"<xml/>"),
                (b"Comp", b"first"),
                (b"Comp", b"second"),
            ],
        );
        assert_eq!(read(&file).unwrap().component, b"first");
    }

    #[test]
    fn an_empty_chunk_is_read_as_empty() {
        let file = preset_file(CLASS, &[(b"Comp", b"")]);
        assert_eq!(read(&file).unwrap().component, b"");
    }

    #[test]
    fn the_class_id_is_compared_without_regard_to_case() {
        let file = preset_file(&CLASS.to_lowercase(), &[(b"Comp", b"x")]);
        assert!(read(&file).is_ok());
    }

    #[test]
    fn the_version_is_ignored_like_the_sdk_does() {
        let file = patched(valid(), 4, &7i32.to_le_bytes());
        assert!(read(&file).is_ok());
    }

    #[test]
    fn a_wrong_signature_is_not_a_preset_file() {
        assert_eq!(
            read(&patched(valid(), 0, b"VST2")),
            Err(PresetFileError::NotAPresetFile)
        );
        assert_eq!(read(b"hello"), Err(PresetFileError::NotAPresetFile));
        assert_eq!(read(b""), Err(PresetFileError::NotAPresetFile));
    }

    #[test]
    fn a_preset_of_another_plugin_is_refused() {
        let file = preset_file("00000000000000000000000000000001", &[(b"Comp", b"x")]);
        assert_eq!(read(&file), Err(PresetFileError::OtherPlugin));
    }

    #[test]
    fn a_class_id_that_is_not_hexadecimal_is_corrupt() {
        let file = patched(valid(), 8, b"ZZ");
        assert!(matches!(read(&file), Err(PresetFileError::Corrupt(_))));
    }

    #[test]
    fn a_file_without_component_state_is_refused() {
        let file = preset_file(CLASS, &[(b"Cont", b"controller")]);
        assert_eq!(read(&file), Err(PresetFileError::NoComponentState));
    }

    #[test]
    fn a_truncated_file_is_refused_at_every_length() {
        let file = valid();
        for length in 0..file.len() {
            assert!(read(&file[..length]).is_err(), "length {length}");
        }
    }

    #[test]
    fn a_list_offset_outside_the_file_is_refused() {
        for offset in [
            -1i64,
            0,
            10,
            47,
            file_len() as i64,
            file_len() as i64 + 1,
            i64::MAX,
        ] {
            let file = patched(valid(), 40, &offset.to_le_bytes());
            assert!(read(&file).is_err(), "offset {offset}");
        }
    }

    fn file_len() -> usize {
        valid().len()
    }

    #[test]
    fn a_missing_list_marker_is_corrupt() {
        let file = valid();
        let at = list_offset(&file);
        assert!(matches!(
            read(&patched(file, at, b"Lizt")),
            Err(PresetFileError::Corrupt(_))
        ));
    }

    #[test]
    fn a_negative_or_empty_entry_count_is_corrupt() {
        for count in [-1i32, 0, i32::MIN] {
            let file = valid();
            let at = list_offset(&file) + 4;
            let file = patched(file, at, &count.to_le_bytes());
            assert!(
                matches!(read(&file), Err(PresetFileError::Corrupt(_))),
                "{count}"
            );
        }
    }

    #[test]
    fn an_absurd_entry_count_reads_no_more_than_the_file_holds() {
        let file = valid();
        let at = list_offset(&file) + 4;
        let file = patched(file, at, &i32::MAX.to_le_bytes());
        assert_eq!(read(&file), Err(PresetFileError::Truncated));
    }

    #[test]
    fn only_the_first_128_entries_are_read() {
        let chunks: Vec<Chunk> = std::iter::once((b"Comp", &b"component"[..]))
            .chain(std::iter::repeat((b"Info", &b"x"[..])).take(200))
            .collect();
        let file = preset_file(CLASS, &chunks);
        assert_eq!(read(&file).unwrap().component, b"component");
    }

    #[test]
    fn a_chunk_outside_the_file_is_corrupt() {
        let file = valid();
        let first_entry = list_offset(&file) + LIST_HEADER_SIZE as usize;
        let cases: [(&str, i64, i64); 7] = [
            ("offset in the header", 10, 4),
            ("offset past the end", file.len() as i64, 1),
            ("size past the end", HEADER_SIZE as i64, file.len() as i64),
            ("size overflows", HEADER_SIZE as i64, i64::MAX),
            ("offset + size overflows", i64::MAX, 2),
            ("negative size", HEADER_SIZE as i64, -1),
            ("negative offset", -5, 4),
        ];
        for (name, offset, size) in cases {
            let mut damaged = file.clone();
            damaged[first_entry + 4..first_entry + 12].copy_from_slice(&offset.to_le_bytes());
            damaged[first_entry + 12..first_entry + 20].copy_from_slice(&size.to_le_bytes());
            assert!(
                matches!(read(&damaged), Err(PresetFileError::Corrupt(_))),
                "{name}"
            );
        }
    }

    #[test]
    fn a_huge_declared_chunk_allocates_nothing() {
        let file = valid();
        let first_entry = list_offset(&file) + LIST_HEADER_SIZE as usize;
        let damaged = patched(file, first_entry + 12, &(i64::MAX / 2).to_le_bytes());
        assert!(read(&damaged).is_err());
    }

    #[test]
    fn a_file_over_the_size_cap_is_refused_before_it_is_read() {
        let result = read_chunks(&mut Cursor::new(Vec::new()), MAX_FILE_BYTES + 1, CLASS);
        assert_eq!(result, Err(PresetFileError::TooLarge));
    }

    #[test]
    fn class_id_text_follows_the_sdk_word_order() {
        assert_eq!(class_id_text(&IID_ICOMPONENT), CLASS);
    }

    #[test]
    fn files_on_disk_are_read_and_listed_by_their_header() {
        let directory =
            std::env::temp_dir().join(format!("echoes-vstpreset-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let ours = directory.join("ours.vstpreset");
        let theirs = directory.join("theirs.vstpreset");
        let garbage = directory.join("garbage.vstpreset");
        std::fs::write(&ours, valid()).unwrap();
        std::fs::write(&theirs, preset_file(&"0".repeat(32), &[(b"Comp", b"x")])).unwrap();
        std::fs::write(&garbage, b"not a preset").unwrap();

        assert!(is_preset_for(&ours, CLASS));
        assert!(!is_preset_for(&theirs, CLASS));
        assert!(!is_preset_for(&garbage, CLASS));
        assert!(!is_preset_for(&directory.join("missing.vstpreset"), CLASS));
        assert_eq!(
            read_preset_chunks(&ours, CLASS).unwrap().component,
            b"component"
        );
        assert_eq!(
            read_preset_chunks(&theirs, CLASS),
            Err(PresetFileError::OtherPlugin)
        );
        assert!(matches!(
            read_preset_chunks(&directory.join("missing.vstpreset"), CLASS),
            Err(PresetFileError::Unreadable(_))
        ));

        std::fs::remove_dir_all(&directory).unwrap();
    }
}
