//! The plugin scan outside the Echoes process (ADR-033, step 10): a plugin that crashes
//! or hangs while it is loaded or scanned must not end the application, it is blocked,
//! and the scan of the other plugins finishes.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::plugin_formats::vst3::test_dll::{effect_id, instrument_id, test_vst3_dll};
use crate::plugin_system::host::PluginHost;
use crate::plugin_system::scan::{scanned_manifests, BlockReason, BlockedPlugin, ScanFormat};

use super::sandbox_tests::{artifacts, environment_lock};

/// Variables the VST3 and CLAP scans read to find their folders.
const SEARCH_VARIABLES: [&str; 4] = ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA", "APPDATA"];

/// The crashing example DLLs, built once into their own target directory.
fn crashing_dll(name: &str) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let examples = BUILT.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = workspace.join("target").join("crashing-plugins");
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "test_crashing_plugin", "--examples"])
            .arg("--target-dir")
            .arg(&target)
            .current_dir(workspace)
            .status()
            .expect("run cargo");
        assert!(status.success(), "building test_crashing_plugin failed");
        target.join("debug").join("examples")
    });
    examples.join(format!("{name}.dll"))
}

/// A machine whose only VST3 folder holds the good test plugin, for the length of the
/// test.  The real folders are hidden so the test never loads an installed plugin.
struct ScannedMachine {
    _environment: MutexGuard<'static, ()>,
    root: PathBuf,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl ScannedMachine {
    fn new() -> Self {
        let environment = environment_lock();
        let root = std::env::temp_dir().join(format!("echoes-scan-sandbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("state")).unwrap();
        let saved = SEARCH_VARIABLES
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        for name in SEARCH_VARIABLES {
            std::env::set_var(name, root.join("nothing_here"));
        }
        std::env::set_var("PROGRAMFILES", root.join("program_files"));
        std::env::set_var(
            test_crashing_plugin::CRASH_LOG_VARIABLE,
            root.join("crash.log"),
        );
        let machine = Self {
            _environment: environment,
            root,
            saved,
        };
        std::fs::create_dir_all(machine.vst3_folder()).unwrap();
        machine.install("good", test_vst3_dll());
        machine
    }

    fn vst3_folder(&self) -> PathBuf {
        self.root
            .join("program_files")
            .join("Common Files")
            .join("VST3")
    }

    /// The folder where a host keeps its blocklist, as the application does.
    fn state_folder(&self) -> PathBuf {
        self.root.join("state")
    }

    fn module(&self, name: &str) -> PathBuf {
        self.vst3_folder().join(format!("{name}.vst3"))
    }

    fn install(&self, name: &str, dll: &Path) {
        std::fs::copy(dll, self.module(name)).unwrap();
    }

    fn install_crashers(&self) {
        self.install("crash_on_load", &crashing_dll("crash_on_load"));
        self.install("crash_in_factory", &crashing_dll("crash_in_factory"));
    }

    /// How many times a scan loaded the crashing example `name`.
    fn loads_of(&self, name: &str) -> usize {
        std::fs::read_to_string(self.root.join("crash.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| *line == name)
            .count()
    }
}

impl Drop for ScannedMachine {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        std::env::remove_var(test_crashing_plugin::CRASH_LOG_VARIABLE);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A host as the application starts it: the bridge found, its blocklist in `state`.
fn host(machine: &ScannedMachine) -> PluginHost {
    let mut host = PluginHost::new();
    host.set_bridge_exe(artifacts().0.clone());
    host.set_scan_directory(&machine.state_folder());
    host
}

fn found_ids(host: &PluginHost) -> Vec<String> {
    host.list_plugins().into_iter().map(|plugin| plugin.id).collect()
}

fn blocked_names(host: &PluginHost) -> Vec<String> {
    let mut names: Vec<String> = host
        .blocked_plugins()
        .into_iter()
        .map(|blocked| blocked.name)
        .collect();
    names.sort();
    names
}

#[test]
fn a_plugin_that_crashes_while_scanned_is_blocked_and_the_scan_of_the_others_finishes() {
    let machine = ScannedMachine::new();
    machine.install_crashers();
    let mut host = host(&machine);

    host.scan_plugins();

    let found = found_ids(&host);
    assert!(found.contains(&effect_id()), "the good effect is found");
    assert!(
        found.contains(&instrument_id()),
        "the good instrument is found"
    );
    assert_eq!(machine.loads_of("crash_on_load"), 1, "it was scanned once");
    assert_eq!(
        machine.loads_of("crash_in_factory"),
        1,
        "it was scanned once"
    );
    assert_eq!(
        blocked_names(&host),
        vec!["crash_in_factory", "crash_on_load"]
    );

    host.scan_plugins();

    assert_eq!(
        machine.loads_of("crash_on_load"),
        1,
        "a blocked plugin is not loaded again"
    );
    assert_eq!(
        machine.loads_of("crash_in_factory"),
        1,
        "a blocked plugin is not loaded again"
    );
}

#[test]
fn a_scanned_plugin_is_registered_as_the_in_process_scan_registered_it() {
    let machine = ScannedMachine::new();
    let mut host = host(&machine);
    host.scan_plugins();

    let effect = host.registry().get(&effect_id()).expect("the effect is found");
    assert_eq!(effect.name, "Echoes Test Gain");
    assert_eq!(effect.vendor, "");
    assert_eq!(effect.categories, vec!["VST3"]);
    assert_eq!(effect.num_sidechain_inputs, 1, "the sidechain probe still runs");
    assert!(!effect.is_instrument);
    assert_eq!(effect.path.as_deref(), Some(machine.module("good").as_path()));

    let instrument = host
        .registry()
        .get(&instrument_id())
        .expect("the instrument is found");
    assert_eq!(instrument.categories, vec!["Instrument", "VST3"]);
    assert_eq!(instrument.num_sidechain_inputs, 0, "instruments are not probed");
    assert!(instrument.is_instrument);
}

#[test]
fn a_plugin_that_hangs_while_scanned_is_blocked_when_the_time_limit_passes() {
    let machine = ScannedMachine::new();
    machine.install("hang_in_factory", &crashing_dll("hang_in_factory"));
    let mut host = host(&machine);
    host.set_scan_timeout(Duration::from_secs(2));

    let started = Instant::now();
    host.scan_plugins();

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the scan does not wait for a hung plugin"
    );
    assert!(found_ids(&host).contains(&effect_id()));
    assert_eq!(machine.loads_of("hang_in_factory"), 1);
    assert_eq!(
        host.take_newly_blocked(),
        vec![BlockedPlugin {
            path: machine.module("hang_in_factory"),
            name: "hang_in_factory".into(),
            reason: BlockReason::TimedOut,
        }]
    );
    assert!(host.take_newly_blocked().is_empty(), "announced once");
}

#[test]
fn a_blocked_plugin_stays_blocked_at_the_next_start_until_it_is_updated_or_re_enabled() {
    let machine = ScannedMachine::new();
    machine.install_crashers();

    let mut first_start = host(&machine);
    first_start.scan_plugins();
    assert_eq!(first_start.take_newly_blocked().len(), 2);

    let mut second_start = host(&machine);
    second_start.scan_plugins();
    assert_eq!(machine.loads_of("crash_on_load"), 1, "not met again");
    assert_eq!(machine.loads_of("crash_in_factory"), 1, "not met again");
    assert_eq!(
        blocked_names(&second_start),
        vec!["crash_in_factory", "crash_on_load"],
        "the blocklist was kept"
    );
    assert!(
        second_start.take_newly_blocked().is_empty(),
        "nothing new to announce"
    );
    assert!(found_ids(&second_start).contains(&effect_id()));

    second_start.reenable_plugin(&machine.module("crash_on_load"));
    assert_eq!(machine.loads_of("crash_on_load"), 2, "listed again on request");
    assert_eq!(
        blocked_names(&second_start),
        vec!["crash_in_factory", "crash_on_load"],
        "and blocked again, as it still crashes"
    );
    assert_eq!(second_start.take_newly_blocked().len(), 1);

    machine.install("crash_in_factory", test_vst3_dll());
    let mut third_start = host(&machine);
    third_start.scan_plugins();
    assert_eq!(
        machine.loads_of("crash_in_factory"),
        1,
        "an updated file is a different file"
    );
    assert_eq!(blocked_names(&third_start), vec!["crash_on_load"]);
}

#[test]
fn a_file_listed_before_and_unchanged_is_not_listed_again_at_the_next_start() {
    let machine = ScannedMachine::new();
    let mut first_start = host(&machine);
    first_start.scan_plugins();

    let mut second_start = PluginHost::new();
    second_start.set_bridge_exe(machine.root.join("no_such_bridge.exe"));
    second_start.set_scan_directory(&machine.state_folder());
    second_start.scan_plugins();
    assert!(
        found_ids(&second_start).contains(&effect_id()),
        "found without starting any scan process"
    );

    let mut forced = PluginHost::new();
    forced.set_bridge_exe(machine.root.join("no_such_bridge.exe"));
    forced.set_scan_directory(&machine.state_folder());
    forced.rescan_plugins();
    assert!(
        !found_ids(&forced).contains(&effect_id()),
        "a rescan lists every file again"
    );
}

#[test]
fn a_file_that_is_not_a_plugin_is_skipped_and_not_blocked() {
    let machine = ScannedMachine::new();
    std::fs::write(machine.module("not_a_plugin"), b"just text").unwrap();
    let mut host = host(&machine);

    host.scan_plugins();

    assert!(host.blocked_plugins().is_empty());
    assert!(found_ids(&host).contains(&effect_id()));
}

#[test]
fn a_scanned_clap_plugin_reports_its_sidechain_like_the_in_process_scan_did() {
    let manifests = scanned_manifests(ScanFormat::Clap, &artifacts().1);

    let gain = manifests
        .iter()
        .find(|manifest| manifest.id == "clap:echoes.test.gain")
        .expect("the test plugin is found");
    assert_eq!(gain.categories, vec!["CLAP"]);
    assert_eq!(
        gain.num_sidechain_inputs, 1,
        "one main and one auxiliary input port"
    );
}
