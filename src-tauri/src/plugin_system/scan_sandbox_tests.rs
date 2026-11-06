//! The plugin scan outside the Echoes process (ADR-033, step 10): a plugin that crashes
//! or hangs while it is loaded or scanned must not end the application, it is blocked,
//! and the scan of the other plugins finishes.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::plugin_formats::vst3::test_dll::{effect_id, instrument_id, test_vst3_dll};
use crate::plugin_system::scan::{
    scanned_manifests, BlockReason, BlockedPlugin, PluginScanner, ScanFormat, SCAN_TIMEOUT,
};
use crate::project::AppState;

use super::sandbox_tests::{artifacts, environment_lock};

/// Variables the VST3 and CLAP scans read to find their folders.
const SEARCH_VARIABLES: [&str; 4] = [
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "LOCALAPPDATA",
    "APPDATA",
];

/// The crashing example DLLs, built once into their own target directory.
fn crashing_dll(name: &str) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let examples = BUILT.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = workspace.join("target").join("crashing-plugins");
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "--quiet",
                "-p",
                "test_crashing_plugin",
                "--examples",
            ])
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

    fn wait_until_loaded(&self, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.loads_of(name) == 0 {
            assert!(
                Instant::now() < deadline,
                "{name} was never loaded by the scan"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
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

/// The application as it starts: the bridge found, its blocklist in the state folder.
fn application(machine: &ScannedMachine, timeout: Duration) -> AppState {
    let mut state = AppState::new();
    state.plugin_scanner = PluginScanner::new(Some(artifacts().0.clone()), timeout);
    state.plugin_scanner.set_directory(&machine.state_folder());
    state
}

fn found_ids(state: &AppState) -> Vec<String> {
    let engine = state.audio_engine.lock().unwrap();
    engine
        .plugin_host
        .list_plugins()
        .into_iter()
        .map(|plugin| plugin.id)
        .collect()
}

fn blocked_names(state: &AppState) -> Vec<String> {
    let mut names: Vec<String> = state
        .plugin_scanner
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
    let state = application(&machine, SCAN_TIMEOUT);

    state.scan_plugins();

    let found = found_ids(&state);
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
        blocked_names(&state),
        vec!["crash_in_factory", "crash_on_load"]
    );

    state.scan_plugins();

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
    let state = application(&machine, SCAN_TIMEOUT);
    state.scan_plugins();

    let engine = state.audio_engine.lock().unwrap();
    let registry = engine.plugin_host.registry();
    let effect = registry.get(&effect_id()).expect("the effect is found");
    assert_eq!(effect.name, "Echoes Test Gain");
    assert_eq!(effect.vendor, "");
    assert_eq!(effect.categories, vec!["VST3"]);
    assert_eq!(
        effect.num_sidechain_inputs, 1,
        "the sidechain probe still runs"
    );
    assert!(!effect.is_instrument);
    assert_eq!(
        effect.path.as_deref(),
        Some(machine.module("good").as_path())
    );

    let instrument = registry
        .get(&instrument_id())
        .expect("the instrument is found");
    assert_eq!(instrument.categories, vec!["Instrument", "VST3"]);
    assert_eq!(
        instrument.num_sidechain_inputs, 0,
        "instruments are not probed"
    );
    assert!(instrument.is_instrument);
}

#[test]
fn a_plugin_that_hangs_while_scanned_is_blocked_when_the_time_limit_passes() {
    let machine = ScannedMachine::new();
    machine.install("hang_in_factory", &crashing_dll("hang_in_factory"));
    let state = application(&machine, Duration::from_secs(2));

    let started = Instant::now();
    state.scan_plugins();

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the scan does not wait for a hung plugin"
    );
    assert!(found_ids(&state).contains(&effect_id()));
    assert_eq!(machine.loads_of("hang_in_factory"), 1);
    assert_eq!(
        state.plugin_scanner.take_newly_blocked(),
        vec![BlockedPlugin {
            path: machine.module("hang_in_factory"),
            name: "hang_in_factory".into(),
            reason: BlockReason::TimedOut,
        }]
    );
    assert!(
        state.plugin_scanner.take_newly_blocked().is_empty(),
        "announced once"
    );
}

#[test]
fn a_blocked_plugin_stays_blocked_at_the_next_start_until_it_is_updated_or_re_enabled() {
    let machine = ScannedMachine::new();
    machine.install_crashers();

    let first_start = application(&machine, SCAN_TIMEOUT);
    first_start.scan_plugins();
    assert_eq!(first_start.plugin_scanner.take_newly_blocked().len(), 2);

    let second_start = application(&machine, SCAN_TIMEOUT);
    second_start.scan_plugins();
    assert_eq!(machine.loads_of("crash_on_load"), 1, "not met again");
    assert_eq!(machine.loads_of("crash_in_factory"), 1, "not met again");
    assert_eq!(
        blocked_names(&second_start),
        vec!["crash_in_factory", "crash_on_load"],
        "the blocklist was kept"
    );
    assert!(
        second_start.plugin_scanner.take_newly_blocked().is_empty(),
        "nothing new to announce"
    );
    assert!(found_ids(&second_start).contains(&effect_id()));

    second_start.reenable_plugin(&machine.module("crash_on_load"));
    assert_eq!(
        machine.loads_of("crash_on_load"),
        2,
        "listed again on request"
    );
    assert_eq!(
        blocked_names(&second_start),
        vec!["crash_in_factory", "crash_on_load"],
        "and blocked again, as it still crashes"
    );
    assert_eq!(second_start.plugin_scanner.take_newly_blocked().len(), 1);

    machine.install("crash_in_factory", test_vst3_dll());
    let third_start = application(&machine, SCAN_TIMEOUT);
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
    application(&machine, SCAN_TIMEOUT).scan_plugins();

    let mut second_start = AppState::new();
    second_start.plugin_scanner =
        PluginScanner::new(Some(machine.root.join("no_such_bridge.exe")), SCAN_TIMEOUT);
    second_start
        .plugin_scanner
        .set_directory(&machine.state_folder());
    second_start.scan_plugins();
    assert!(
        found_ids(&second_start).contains(&effect_id()),
        "found without starting any scan process"
    );
}

#[test]
fn a_file_that_is_not_a_plugin_is_skipped_and_not_blocked() {
    let machine = ScannedMachine::new();
    std::fs::write(machine.module("not_a_plugin"), b"just text").unwrap();
    let state = application(&machine, SCAN_TIMEOUT);

    state.scan_plugins();

    assert!(state.plugin_scanner.blocked_plugins().is_empty());
    assert!(found_ids(&state).contains(&effect_id()));
}

/// A scan on its own thread, stuck on a plugin that hangs.
fn scan_stuck_on_a_hanging_plugin(
    machine: &ScannedMachine,
    timeout: Duration,
) -> (Arc<AppState>, JoinHandle<()>) {
    machine.install("hang_in_factory", &crashing_dll("hang_in_factory"));
    let state = Arc::new(application(machine, timeout));
    let scan = {
        let state = Arc::clone(&state);
        std::thread::spawn(move || state.scan_plugins())
    };
    machine.wait_until_loaded("hang_in_factory");
    (state, scan)
}

#[test]
fn the_engine_stays_free_while_a_plugin_hangs_during_the_scan() {
    let machine = ScannedMachine::new();
    let (state, scan) = scan_stuck_on_a_hanging_plugin(&machine, Duration::from_secs(6));

    let started = Instant::now();
    assert!(state.plugin_scanner.is_scanning());
    assert!(
        state.audio_engine.try_lock().is_ok(),
        "the engine mutex is free while a scan process hangs"
    );
    assert!(
        found_ids(&state).contains(&effect_id()),
        "a plugin listed before the hung file is usable already"
    );
    assert!(state.plugin_scanner.blocked_plugins().is_empty());
    assert!(state.plugin_scanner.take_newly_blocked().is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "none of this waits for the scan"
    );

    scan.join().unwrap();
    assert!(!state.plugin_scanner.is_scanning());
    assert_eq!(blocked_names(&state), vec!["hang_in_factory"]);
}

#[test]
fn a_scan_started_during_another_waits_for_it_and_does_not_load_the_hung_plugin_again() {
    let machine = ScannedMachine::new();
    let (state, first) = scan_stuck_on_a_hanging_plugin(&machine, Duration::from_secs(4));
    let second = {
        let state = Arc::clone(&state);
        std::thread::spawn(move || state.scan_plugins())
    };

    first.join().unwrap();
    second.join().unwrap();

    assert_eq!(
        machine.loads_of("hang_in_factory"),
        1,
        "the second scan came after the block"
    );
    assert_eq!(blocked_names(&state), vec!["hang_in_factory"]);
}

#[test]
fn re_enabling_a_plugin_during_a_scan_waits_for_the_scan() {
    let machine = ScannedMachine::new();
    machine.install("crash_on_load", &crashing_dll("crash_on_load"));
    application(&machine, SCAN_TIMEOUT).scan_plugins();
    assert_eq!(machine.loads_of("crash_on_load"), 1);

    let (state, scan) = scan_stuck_on_a_hanging_plugin(&machine, Duration::from_secs(4));
    let started = Instant::now();
    state.reenable_plugin(&machine.module("crash_on_load"));

    assert!(
        started.elapsed() > Duration::from_secs(1),
        "the re-enable came after the scan, not beside it"
    );
    scan.join().unwrap();
    assert_eq!(machine.loads_of("crash_on_load"), 2, "listed again once");
}

#[cfg(windows)]
#[test]
fn the_scan_processes_end_with_the_job_they_belong_to() {
    let machine = ScannedMachine::new();
    let (state, scan) = scan_stuck_on_a_hanging_plugin(&machine, Duration::from_secs(60));

    let started = Instant::now();
    state.plugin_scanner.terminate_scan_processes();
    scan.join().unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the hung scan process was ended, not waited for"
    );
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
