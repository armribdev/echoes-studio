//! The presets of a VST3 plugin in the sandbox (ADR-033, step 9): the test effect's
//! programs and a `.vstpreset` file, offered and loaded in the bridge process and in the
//! Echoes process, which must answer the same.  The audio thread is played by a thread
//! of the test, since a program reaches the processor with a block.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use test_vst3_plugin::{GAIN, PROGRAM, SELF_VALUE};

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, PrepareContext, TransportInfo};
use crate::plugin_formats::vst3::host::decode_plugin_id;
use crate::plugin_formats::vst3::preset_file::class_id_text;
use crate::plugin_formats::vst3::preset_file::tests::preset_file;
use crate::plugin_formats::vst3::preset_locations::PresetRoots;
use crate::plugin_formats::vst3::test_dll::{effect_id, test_vst3_dll};
use crate::plugin_system::host::PluginHost;
use crate::plugin_system::presets::{PluginPreset, PresetListing, Vst3PresetFiles};
use crate::plugin_system::registry::PluginManifest;
use crate::plugin_system::wrapper::DeviceFormat;

use super::sandbox_tests::{artifacts, environment_lock};

const KEY: &str = "fx0";
const FRAMES: usize = 64;
const PLUGIN_NAME: &str = "Echoes Test Gain";
const FILE_GAIN: f64 = 0.5;
const FILE_SELF_VALUE: f64 = 0.8;

fn host() -> PluginHost {
    let mut host = PluginHost::new();
    host.set_bridge_exe(artifacts().0.clone());
    host.registry_mut().add(PluginManifest {
        id: effect_id(),
        name: PLUGIN_NAME.into(),
        vendor: "Echoes Studio".into(),
        format: DeviceFormat::Vst3,
        path: Some(test_vst3_dll().clone()),
        categories: vec![],
        num_sidechain_inputs: 0,
        is_instrument: false,
    });
    host
}

/// A folder of the test's own that is removed with it.
struct Scratch(PathBuf);

impl Scratch {
    fn new(sandboxed: bool) -> Self {
        let path = std::env::temp_dir().join(format!(
            "echoes-vst3-preset-sandbox-{}-{}",
            if sandboxed { "bridge" } else { "process" },
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    /// A `.vstpreset` of the test effect with the component state and the controller's.
    fn bright_preset(&self) -> String {
        let class_id = class_id_text(&decode_plugin_id(&effect_id()).unwrap());
        let file = preset_file(
            &class_id,
            &[
                (b"Comp", &FILE_GAIN.to_le_bytes()),
                (b"Cont", &FILE_SELF_VALUE.to_le_bytes()),
            ],
        );
        let path = self
            .0
            .join("docs/VST3 Presets/Echoes Studio")
            .join(PLUGIN_NAME)
            .join("Bright.vstpreset");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, file).unwrap();
        path.to_str().unwrap().to_owned()
    }

    fn roots(&self) -> PresetRoots {
        PresetRoots {
            documents: Some(self.0.join("docs")),
            app_data: None,
            program_data: None,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_block(node: &mut Box<dyn AudioNode>) -> Vec<f32> {
    let mut input = AudioBuffer::new(2, 4096);
    for channel in 0..2 {
        input.channel_mut(channel)[..FRAMES].fill(0.5);
    }
    let mut outputs = vec![AudioBuffer::new(2, 4096)];
    let transport = TransportInfo {
        block_size: FRAMES,
        ..TransportInfo::stopped(48_000.0)
    };
    node.process(&[&input], &mut outputs, &transport);
    outputs[0].channel(0)[..FRAMES].to_vec()
}

/// Plays blocks on a thread of its own while `during` runs, as the audio thread does.
fn while_audio_runs<T>(
    node: Box<dyn AudioNode>,
    during: impl FnOnce() -> T,
) -> (Box<dyn AudioNode>, T) {
    let stop = Arc::new(AtomicBool::new(false));
    let audio = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut node = node;
            while !stop.load(Ordering::Relaxed) {
                run_block(&mut node);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            node
        })
    };
    let result = during();
    stop.store(true, Ordering::Relaxed);
    (audio.join().expect("the audio thread ends"), result)
}

/// What a user sees of a plugin's presets, and what they did to the plugin.
#[derive(Debug, PartialEq)]
struct Observed {
    /// Name, "in use", provider.
    listed: Vec<(String, bool, String)>,
    after_program: Effect,
    listed_after_program: Vec<(String, bool)>,
    after_file: Effect,
}

#[derive(Debug, PartialEq)]
struct Effect {
    gain: Option<f64>,
    program: Option<f64>,
    self_value: Option<f64>,
    saved_state: Option<Vec<u8>>,
    heard: Vec<f32>,
    marked_unsaved: u32,
}

fn effect(host: &PluginHost, node: &mut Box<dyn AudioNode>) -> Effect {
    let instance = host.instance_by_key(KEY).unwrap();
    Effect {
        gain: instance.param_value(GAIN),
        program: instance.param_value(PROGRAM),
        self_value: instance.param_value(SELF_VALUE),
        saved_state: host
            .collect_plugin_states()
            .into_iter()
            .find(|(key, _)| key == KEY)
            .map(|(_, bytes)| bytes),
        heard: run_block(node),
        marked_unsaved: instance.state_edit_count(),
    }
}

fn load(host: &PluginHost, preset: &PluginPreset) -> Result<(), String> {
    host.instance_for_preset(KEY, &preset.request())?
        .load_preset(&preset.request())
}

fn named<'a>(presets: &'a [PluginPreset], name: &str) -> &'a PluginPreset {
    presets
        .iter()
        .find(|preset| preset.name == name)
        .unwrap_or_else(|| panic!("the plugin offers no preset named {name}: {presets:?}"))
}

fn summary(presets: &[PluginPreset]) -> Vec<(String, bool)> {
    presets
        .iter()
        .map(|preset| (preset.name.clone(), preset.active))
        .collect()
}

fn observe(sandboxed: bool) -> Observed {
    let scratch = Scratch::new(sandboxed);
    scratch.bright_preset();
    let mut host = host();
    let mut node = host
        .instantiate(KEY, &effect_id(), sandboxed)
        .expect("plugin starts");
    node.prepare(PrepareContext {
        sample_rate: 48_000.0,
        max_block_size: 4096,
        num_channels: 2,
        live: false,
    });
    assert_eq!(
        host.instance_by_key(KEY).unwrap().is_sandboxed(),
        sandboxed,
        "the plugin must really run where the test says"
    );

    assert!(
        host.preset_listing(KEY).is_some(),
        "a VST3 plugin offers presets (sandboxed: {sandboxed})"
    );
    let listing = PresetListing::Vst3 {
        instance: host.instance_by_key(KEY).unwrap(),
        files: Some(
            Vst3PresetFiles::new(PLUGIN_NAME, &effect_id())
                .unwrap()
                .under(scratch.roots()),
        ),
    };
    let listed = listing.list();
    host.remember_listing(&listing, listed.clone());
    let observed_listing = listed
        .iter()
        .map(|preset| {
            (
                preset.name.clone(),
                preset.active,
                preset.provider_id.clone(),
            )
        })
        .collect();

    let (mut node, chosen) = while_audio_runs(node, || load(&host, named(&listed, "Loud")));
    chosen.expect("the program loads");
    let after_program = effect(&host, &mut node);
    let listed_after_program = summary(&listing.list());

    let (mut node, loaded) = while_audio_runs(node, || load(&host, named(&listed, "Bright")));
    loaded.expect("the file loads");
    let after_file = effect(&host, &mut node);

    Observed {
        listed: observed_listing,
        after_program,
        listed_after_program,
        after_file,
    }
}

#[test]
fn a_sandboxed_plugin_offers_and_loads_its_presets_like_an_in_process_one() {
    let _environment = environment_lock();
    let in_process = observe(false);

    assert_eq!(
        in_process
            .listed
            .iter()
            .map(|(name, active, _)| (name.as_str(), *active))
            .collect::<Vec<_>>(),
        [
            ("Quiet", false),
            ("Unity", true),
            ("Loud", false),
            ("Bright", false)
        ],
        "the programs in the plugin's order, the default one in use, then the file"
    );
    assert_eq!(in_process.after_program.gain, Some(1.75 / 2.0));
    assert_eq!(in_process.after_program.program, Some(1.0));
    assert_eq!(in_process.after_program.heard, vec![0.5 * 1.75; FRAMES]);
    assert_eq!(
        in_process.listed_after_program,
        [
            ("Quiet".to_string(), false),
            ("Unity".to_string(), false),
            ("Loud".to_string(), true),
            ("Bright".to_string(), false)
        ]
    );
    assert_eq!(in_process.after_file.gain, Some(FILE_GAIN / 2.0));
    assert_eq!(in_process.after_file.self_value, Some(FILE_SELF_VALUE));
    assert_eq!(
        in_process.after_file.heard,
        vec![0.5 * FILE_GAIN as f32; FRAMES]
    );
    assert!(in_process.after_file.marked_unsaved > in_process.after_program.marked_unsaved);

    assert_eq!(observe(true), in_process);
}

fn sandboxed_host() -> (PluginHost, Box<dyn AudioNode>) {
    let mut host = host();
    let mut node = host
        .instantiate(KEY, &effect_id(), true)
        .expect("plugin starts");
    node.prepare(PrepareContext {
        sample_rate: 48_000.0,
        max_block_size: 4096,
        num_channels: 2,
        live: false,
    });
    assert!(host.instance_by_key(KEY).unwrap().is_sandboxed());
    (host, node)
}

#[test]
fn a_sandboxed_plugin_loads_only_what_its_last_listing_offered() {
    let _environment = environment_lock();
    let (mut host, _node) = sandboxed_host();
    let scratch = Scratch::new(true);
    let unlisted = scratch.bright_preset();
    let listing = host.preset_listing(KEY).expect("a listing");
    host.remember_listing(&listing, listing.list());

    let file = PluginPreset::vst3_file(
        "Bright".into(),
        unlisted,
        crate::plugin_system::presets::PresetOrigin::User,
    );
    assert!(load(&host, &file).is_err(), "a file nobody offered");
    let guessed = PluginPreset {
        load_key: Some("23:0".into()),
        location_kind: crate::plugin_formats::clap::ffi::CLAP_PRESET_LOCATION_PLUGIN,
        location: None,
        ..file.clone()
    };
    assert!(
        load(&host, &guessed).is_ok(),
        "the programs of the listing are offered"
    );
    let outside = PluginPreset {
        load_key: Some("23:9".into()),
        ..guessed
    };
    assert!(load(&host, &outside).is_err(), "a program past the list");
}

#[test]
fn a_file_the_engine_cannot_use_is_refused_before_the_bridge_is_touched() {
    let (host, mut node) = sandboxed_host();
    let scratch = Scratch::new(true);
    let broken = scratch.bright_preset();
    std::fs::write(&broken, b"VST3 not a preset").unwrap();
    let instance = host.instance_by_key(KEY).unwrap();
    let marks = instance.state_edit_count();
    let before = run_block(&mut node);

    let file = PluginPreset::vst3_file(
        "Broken".into(),
        broken,
        crate::plugin_system::presets::PresetOrigin::User,
    );
    assert!(instance.load_preset(&file.request()).is_err());

    assert_eq!(instance.state_edit_count(), marks, "nothing was loaded");
    assert_eq!(run_block(&mut node), before);
    assert!(!instance.has_crashed());
}
