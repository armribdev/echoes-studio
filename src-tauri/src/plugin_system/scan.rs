//! The plugin scan outside the Echoes process (ADR-033, step 10).
//!
//! Each plugin file is listed by `plugin_host_bridge --scan` in a process of its own
//! with a time limit.  A file that crashes or hangs ends only that process and is
//! written to the blocklist, so the scan of the others finishes and the next start does
//! not meet it again.  A file is blocked until it changes (size or date) or the user
//! re-enables it.  The blocklist and the results of the last scan are kept per machine,
//! next to the crash marker (ADR-015).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use plugin_bridge::records::{ScannedPluginRecord, SCAN_EXIT_REFUSED};
use serde::{Deserialize, Serialize};

use crate::plugin_system::registry::PluginManifest;
use crate::plugin_system::wrapper::DeviceFormat;

/// Longest a file may take to be listed, including a cold start of a large plugin.
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// A plugin may have handed the output stream to a process of its own that outlives the
/// scan; its output is not waited for past this.
const OUTPUT_GRACE: Duration = Duration::from_secs(1);

const BLOCKLIST_FILE: &str = "plugin-blocklist.json";
const SCAN_CACHE_FILE: &str = "plugin-scan-cache.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanFormat {
    Vst3,
    Clap,
}

impl ScanFormat {
    fn argument(self) -> &'static str {
        match self {
            Self::Vst3 => "vst3",
            Self::Clap => "clap",
        }
    }

    fn device_format(self) -> DeviceFormat {
        match self {
            Self::Vst3 => DeviceFormat::Vst3,
            Self::Clap => DeviceFormat::Clap,
        }
    }

    fn category(self) -> &'static str {
        match self {
            Self::Vst3 => "VST3",
            Self::Clap => "CLAP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    Crashed,
    TimedOut,
}

/// What became of one plugin file.
#[derive(Debug, PartialEq, Eq)]
pub enum FileScan {
    Plugins(Vec<ScannedPluginRecord>),
    /// The file could not be loaded as a plugin of this machine (wrong architecture, a
    /// missing dependency).  Not the plugin's crash, so not blocked.
    Refused,
    Blocked(BlockReason),
}

/// Lists the plugins of `file` in a scan process and waits at most `timeout`.
pub fn scan_file(bridge_exe: &Path, format: ScanFormat, file: &Path, timeout: Duration) -> FileScan {
    let mut command = Command::new(bridge_exe);
    command
        .arg("--scan")
        .arg(format.argument())
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("[plugin] scan: cannot start {}: {error}", bridge_exe.display());
            return FileScan::Refused;
        }
    };

    let mut stdout = child.stdout.take().expect("the output is piped");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return FileScan::Blocked(BlockReason::TimedOut);
            }
            Err(error) => {
                eprintln!("[plugin] scan: cannot wait for the scan of {}: {error}", file.display());
                let _ = child.kill();
                return FileScan::Refused;
            }
        }
    };

    match status.code() {
        Some(0) => receiver
            .recv_timeout(OUTPUT_GRACE)
            .ok()
            .and_then(|bytes| {
                ScannedPluginRecord::from_scan_output(&String::from_utf8_lossy(&bytes)).ok()
            })
            .map_or(FileScan::Blocked(BlockReason::Crashed), FileScan::Plugins),
        Some(code) if code == i32::from(SCAN_EXIT_REFUSED) => FileScan::Refused,
        _ => FileScan::Blocked(BlockReason::Crashed),
    }
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

/// The device the registry knows for a plugin a scan found in `module`.
pub fn manifest_of(format: ScanFormat, module: &Path, plugin: &ScannedPluginRecord) -> PluginManifest {
    let id_prefix = match format {
        ScanFormat::Vst3 => "vst3",
        ScanFormat::Clap => "clap",
    };
    let categories = if plugin.is_instrument {
        vec!["Instrument".into(), format.category().into()]
    } else {
        vec![format.category().into()]
    };
    PluginManifest {
        id: format!("{id_prefix}:{}", plugin.native_id),
        name: plugin.name.clone(),
        vendor: plugin.vendor.clone(),
        format: format.device_format(),
        path: Some(module.to_path_buf()),
        categories,
        num_sidechain_inputs: plugin.sidechain_inputs as usize,
        is_instrument: plugin.is_instrument,
    }
}

/// The plugins of one module, scanned through the bridge the way the application does.
#[cfg(all(test, windows))]
pub(crate) fn scanned_manifests(format: ScanFormat, module: &Path) -> Vec<PluginManifest> {
    let bridge_exe = &crate::plugin_system::sandbox_tests::artifacts().0;
    let module = match format {
        ScanFormat::Vst3 => match crate::plugin_formats::vst3::host::resolve_vst3_dll(module) {
            Some(dll) => dll,
            None => return Vec::new(),
        },
        ScanFormat::Clap => module.to_path_buf(),
    };
    match scan_file(bridge_exe, format, &module, SCAN_TIMEOUT) {
        FileScan::Plugins(plugins) => plugins
            .iter()
            .map(|plugin| manifest_of(format, &module, plugin))
            .collect(),
        other => panic!("scan of {} gave {other:?}", module.display()),
    }
}

/// Size and modification time: a file that changed is not the file that was blocked or
/// listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    size: u64,
    modified_nanos: u64,
}

impl FileStamp {
    pub fn of(file: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(file).ok()?;
        let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            size: metadata.len(),
            modified_nanos: u64::try_from(modified.as_nanos()).ok()?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlockedEntry {
    path: PathBuf,
    format: ScanFormat,
    stamp: FileStamp,
    reason: BlockReason,
}

/// A scanned file, with the plugins it held as the hexadecimal result line of the scan
/// process (`plugin_bridge` has no serde).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScannedEntry {
    path: PathBuf,
    stamp: FileStamp,
    plugins: String,
}

/// A blocked file, as the interface shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockedPlugin {
    pub path: PathBuf,
    pub name: String,
    pub reason: BlockReason,
}

/// The blocklist and the results of the last scan.  Without a directory (tests) it lives
/// in memory only.
#[derive(Default)]
pub struct ScanStore {
    directory: Option<PathBuf>,
    blocked: Vec<BlockedEntry>,
    scanned: Vec<ScannedEntry>,
}

impl ScanStore {
    /// Reads what an earlier run saved; an unreadable file counts as empty.
    pub fn open(directory: &Path) -> Self {
        Self {
            directory: Some(directory.to_path_buf()),
            blocked: read_json(&directory.join(BLOCKLIST_FILE)),
            scanned: read_json(&directory.join(SCAN_CACHE_FILE)),
        }
    }

    pub fn is_blocked(&self, file: &Path, stamp: FileStamp) -> bool {
        self.blocked
            .iter()
            .any(|entry| entry.path == file && entry.stamp == stamp)
    }

    /// Blocks `file`; returns it as the interface shows it.
    pub fn block(
        &mut self,
        file: &Path,
        format: ScanFormat,
        stamp: FileStamp,
        reason: BlockReason,
    ) -> BlockedPlugin {
        self.unblock(file);
        let entry = BlockedEntry {
            path: file.to_path_buf(),
            format,
            stamp,
            reason,
        };
        let plugin = blocked_plugin(&entry);
        self.blocked.push(entry);
        plugin
    }

    /// The format of a blocked file; `None` if it is not blocked.
    pub fn unblock(&mut self, file: &Path) -> Option<ScanFormat> {
        let index = self.blocked.iter().position(|entry| entry.path == file)?;
        Some(self.blocked.remove(index).format)
    }

    pub fn blocked_plugins(&self) -> Vec<BlockedPlugin> {
        self.blocked.iter().map(blocked_plugin).collect()
    }

    pub fn cached(&self, file: &Path, stamp: FileStamp) -> Option<Vec<ScannedPluginRecord>> {
        let entry = self
            .scanned
            .iter()
            .find(|entry| entry.path == file && entry.stamp == stamp)?;
        ScannedPluginRecord::from_scan_output(&entry.plugins).ok()
    }

    pub fn remember(&mut self, file: &Path, stamp: FileStamp, plugins: &[ScannedPluginRecord]) {
        self.scanned.retain(|entry| entry.path != file);
        self.scanned.push(ScannedEntry {
            path: file.to_path_buf(),
            stamp,
            plugins: ScannedPluginRecord::result_line(plugins),
        });
        self.unblock(file);
    }

    pub fn forget_scanned(&mut self) {
        self.scanned.clear();
    }

    /// Drops what is known of files a scan no longer found.
    pub fn keep_only(&mut self, files: &[PathBuf]) {
        self.scanned.retain(|entry| files.contains(&entry.path));
        self.blocked.retain(|entry| files.contains(&entry.path));
    }

    /// Best effort: losing the cache costs a scan, losing the blocklist a second crash.
    pub fn save(&self) {
        let Some(directory) = &self.directory else {
            return;
        };
        write_json(&directory.join(BLOCKLIST_FILE), &self.blocked);
        write_json(&directory.join(SCAN_CACHE_FILE), &self.scanned);
    }
}

fn blocked_plugin(entry: &BlockedEntry) -> BlockedPlugin {
    BlockedPlugin {
        path: entry.path.clone(),
        name: entry
            .path
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        reason: entry.reason,
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, entries: &[T]) {
    let written = serde_json::to_vec_pretty(entries)
        .map_err(|error| error.to_string())
        .and_then(|bytes| std::fs::write(path, bytes).map_err(|error| error.to_string()));
    if let Err(error) = written {
        eprintln!("[plugin] scan: cannot write {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("echoes-scan-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn stamp(size: u64) -> FileStamp {
        FileStamp {
            size,
            modified_nanos: 7,
        }
    }

    fn plugin(native_id: &str) -> ScannedPluginRecord {
        ScannedPluginRecord {
            native_id: native_id.into(),
            name: "Plugin".into(),
            vendor: String::new(),
            is_instrument: false,
            sidechain_inputs: 1,
        }
    }

    #[test]
    fn a_blocked_file_stays_blocked_across_runs_until_it_changes_or_is_re_enabled() {
        let directory = scratch("blocked");
        let file = Path::new("C:/Plugins/Broken.vst3");
        let mut store = ScanStore::open(&directory);
        store.block(file, ScanFormat::Vst3, stamp(10), BlockReason::Crashed);
        store.save();

        let mut next_run = ScanStore::open(&directory);
        assert!(next_run.is_blocked(file, stamp(10)));
        assert!(!next_run.is_blocked(file, stamp(11)), "an updated file is tried again");
        assert_eq!(
            next_run.blocked_plugins(),
            vec![BlockedPlugin {
                path: file.to_path_buf(),
                name: "Broken".into(),
                reason: BlockReason::Crashed,
            }]
        );

        assert_eq!(next_run.unblock(file), Some(ScanFormat::Vst3));
        assert_eq!(next_run.unblock(file), None);
        assert!(!next_run.is_blocked(file, stamp(10)));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_listed_file_is_remembered_until_it_changes_and_forgotten_when_gone() {
        let directory = scratch("cache");
        let kept = Path::new("C:/Plugins/Kept.vst3");
        let gone = Path::new("C:/Plugins/Gone.vst3");
        let mut store = ScanStore::open(&directory);
        store.remember(kept, stamp(1), &[plugin("A")]);
        store.remember(gone, stamp(2), &[plugin("B")]);
        store.keep_only(&[kept.to_path_buf()]);
        store.save();

        let next_run = ScanStore::open(&directory);
        assert_eq!(next_run.cached(kept, stamp(1)), Some(vec![plugin("A")]));
        assert_eq!(next_run.cached(kept, stamp(2)), None, "a changed file is listed again");
        assert_eq!(next_run.cached(gone, stamp(2)), None);

        let mut cleared = next_run;
        cleared.forget_scanned();
        assert_eq!(cleared.cached(kept, stamp(1)), None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_file_that_lists_fine_is_no_longer_blocked() {
        let file = Path::new("C:/Plugins/Fixed.vst3");
        let mut store = ScanStore::default();
        store.block(file, ScanFormat::Vst3, stamp(1), BlockReason::TimedOut);
        store.remember(file, stamp(2), &[plugin("A")]);
        assert!(store.blocked_plugins().is_empty());
    }

    #[test]
    fn unreadable_saved_files_count_as_empty() {
        let directory = scratch("garbage");
        std::fs::write(directory.join(BLOCKLIST_FILE), b"not json").unwrap();
        std::fs::write(directory.join(SCAN_CACHE_FILE), b"{}").unwrap();
        let store = ScanStore::open(&directory);
        assert!(store.blocked_plugins().is_empty());
        assert_eq!(store.cached(Path::new("x"), stamp(1)), None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_manifest_of_a_scanned_plugin_names_it_like_the_old_scan_did() {
        let module = Path::new("C:/Plugins/Synth.vst3");
        let effect = manifest_of(ScanFormat::Vst3, module, &plugin("00FF"));
        assert_eq!(effect.id, "vst3:00FF");
        assert_eq!(effect.categories, vec!["VST3"]);
        assert_eq!(effect.num_sidechain_inputs, 1);
        assert_eq!(effect.path.as_deref(), Some(module));

        let instrument = ScannedPluginRecord {
            is_instrument: true,
            sidechain_inputs: 0,
            ..plugin("a.b.c")
        };
        let instrument = manifest_of(ScanFormat::Clap, module, &instrument);
        assert_eq!(instrument.id, "clap:a.b.c");
        assert_eq!(instrument.categories, vec!["Instrument", "CLAP"]);
        assert_eq!(instrument.format, DeviceFormat::Clap);
        assert!(instrument.is_instrument);
    }
}
