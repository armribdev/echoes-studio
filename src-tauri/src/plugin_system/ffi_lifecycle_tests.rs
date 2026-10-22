//! Real-FFI plugin lifecycle harness (validation only — no production code).
//!
//! Closes audit "Lacune A" (real activation runtime) and "Lacune C" (native
//! teardown), which the existing suite only exercises through the internal /
//! pass-through bypass path.  Everything here drives a **real** VST3 / CLAP
//! binary through the engine's own public runtime APIs — no mock, no synthetic
//! vtable:
//!
//!   * instantiation         → `PluginHost::instantiate` (real `LoadLibrary` +
//!     factory `create_plugin` / `createInstance`)
//!   * activation runtime    → `PluginInstance::{clap,vst3}_ensure_active`
//!   * sample-rate / block reinit → the same call at new bounds
//!   * audio block           → the wrapper node's real `process` vtable
//!   * teardown              → last-`Arc` drop → `PluginInstance::Drop` FFI release
//!
//! ## How to run
//!
//! These tests are **gated on a real plugin being present** so CI without
//! plugins stays green: if none is discovered they log `SKIP` and pass.  On a
//! machine with plugins installed:
//!
//! ```text
//! cargo test -p echoes-studio ffi_lifecycle -- --nocapture --test-threads=1
//! ```
//!
//! Override discovery with an explicit binary:
//!   `ECHOES_TEST_CLAP=<path-to.clap>`  /  `ECHOES_TEST_VST3=<path-to.vst3>`
//!
//! `--test-threads=1` keeps the real-cycle logs from interleaving.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{PrepareContext, TransportInfo};
use crate::plugin_system::host::PluginHost;
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::lifecycle::LifecycleState;
use crate::plugin_system::registry::PluginManifest;
use crate::plugin_system::wrapper::DeviceFormat;

const LOG: &str = "[ffi-harness]";

// ── Discovery (real binaries on disk) ───────────────────────────────────────

fn search_roots(env_dirs: &[&str], leaf: &str) -> Vec<PathBuf> {
    env_dirs
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .map(|p| PathBuf::from(p).join("Common Files").join(leaf))
        .collect()
}

/// Walk up to `depth` levels collecting files with `ext`.  A `.vst3` is reported
/// only as a single-file binary (a bundle directory would need the engine's
/// private `resolve_vst3_dll`, out of scope for a test).
fn collect(root: &Path, ext: &str, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, ext, depth - 1, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        {
            out.push(path);
        }
    }
}

/// All real plugin binaries of `ext` on disk: explicit env override first, then
/// a recursive scan of the platform directories.  For `.vst3` the recursion
/// descends into bundles and finds the inner module (also `.vst3`), so both
/// single-file and bundle plugins are returned as directly-loadable paths.
/// Sorted so an **effect** (filename hint) comes first → a meaningful process
/// block.
fn discover_all(env_var: &str, ext: &str) -> Vec<PathBuf> {
    if let Ok(p) = std::env::var(env_var) {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return vec![pb];
        }
    }
    let roots = search_roots(
        &["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"],
        if ext == "clap" { "CLAP" } else { "VST3" },
    );
    let mut found = Vec::new();
    for root in roots {
        collect(&root, ext, 4, &mut found);
    }
    found.sort();
    found.sort_by_key(|p| {
        let n = p.to_string_lossy().to_lowercase();
        u8::from(!(n.contains("effect") || n.contains("nova")))
    });
    found
}

/// 0x80004002 etc. are clearer as their COM symbol in the logs.
fn hresult_name(r: i32) -> &'static str {
    match r as u32 {
        0x8000_4002 => "E_NOINTERFACE",
        0x8000_4001 => "E_NOTIMPL",
        0x8007_0057 => "E_INVALIDARG",
        0x8000_4005 => "E_FAIL",
        _ => "?",
    }
}

// ── Shared lifecycle driver ─────────────────────────────────────────────────

struct Subject {
    device_id: String,
    instance_key: String,
    node: Box<dyn crate::audio_graph::node::AudioNode>,
    instance: Arc<PluginInstance>,
}

/// Run T1–T5 against an already-instantiated subject. `ensure_active` is the
/// format-specific activation call (clap vs vst3) under test.
fn run_lifecycle(
    mut host: PluginHost,
    subject: Subject,
    ensure_active: impl Fn(&PluginInstance, f32, u32) -> bool,
) {
    let Subject {
        device_id,
        instance_key,
        mut node,
        instance,
    } = subject;

    // T1 — real activation: Created → activate → start_processing.
    let active = ensure_active(&instance, 48_000.0, 512);
    eprintln!("{LOG} T1 activation @48k/512: active={active}");
    assert!(active, "T1: plugin must reach the active+processing state");

    // T2 — real audio block through the plugin's process vtable.
    node.prepare(PrepareContext {
        sample_rate: 48_000.0,
        max_block_size: 512,
        num_channels: 2,
        live: true,
    });
    let frames = 256;
    let mut input = AudioBuffer::new(2, frames);
    for ch in 0..2 {
        for (i, s) in input.channel_mut(ch).iter_mut().enumerate() {
            *s = ((i as f32) * 0.001).sin() * 0.25;
        }
    }
    let inputs = [&input];
    let mut outputs = vec![AudioBuffer::new(2, frames)];
    let transport = TransportInfo {
        is_playing: true,
        position_samples: 0,
        position_seconds: 0.0,
        bpm: 120.0,
        time_signature: crate::audio_graph::node::TimeSignature::default(),
        sample_rate: 48_000.0,
        block_size: frames,
    };
    node.process(&inputs, &mut outputs, &transport);
    let finite = outputs
        .iter()
        .all(|b| (0..2).all(|c| b.channel(c).iter().all(|s| s.is_finite())));
    eprintln!("{LOG} T2 processed {frames} frames: output finite={finite}");
    assert!(finite, "T2: processed output must be finite (no NaN/Inf)");

    // T3 — sample-rate reinit (44.1k): forces deactivate + reactivate.
    let re_sr = ensure_active(&instance, 44_100.0, 512);
    eprintln!("{LOG} T3 sample-rate reinit @44.1k/512: active={re_sr}");
    assert!(re_sr, "T3: reactivation at a new sample rate must succeed");

    // T4 — block-size change (256): re-init at new bounds.
    let re_block = ensure_active(&instance, 44_100.0, 256);
    eprintln!("{LOG} T4 block-size reinit @44.1k/256: active={re_block}");
    assert!(re_block, "T4: re-init at a new block size must succeed");

    // T5 — teardown / no leak.  Drop the audio-side wrapper, prune the host
    // record, then prove the harness holds the *only* remaining Arc — so the
    // final drop is what runs the real FFI release sequence
    // (stop_processing → deactivate → destroy / setActive(0) → terminate → release).
    drop(node);
    host.destroy_device(&device_id);
    assert_eq!(host.device_count(), 0, "T5: host record must be pruned");
    assert_eq!(
        Arc::strong_count(&instance),
        1,
        "T5: no leak — only the harness reference may remain before teardown"
    );
    assert_eq!(
        instance.lifecycle(),
        LifecycleState::Destroyed,
        "T5: instance must be marked Destroyed"
    );
    eprintln!(
        "{LOG} T5 teardown: strong_count=1, lifecycle=Destroyed, key={instance_key} — dropping (real FFI release)"
    );
    drop(instance); // real native teardown; absence of crash/hang == clean release
    eprintln!("{LOG} T5 teardown complete — native handle released, no crash");
}

// ── CLAP ────────────────────────────────────────────────────────────────────

/// First CLAP plugin id (`clap:<id>`) exported by a `.clap`, via the real loader.
fn first_clap_id(path: &Path) -> Option<String> {
    use crate::plugin_formats::clap::ffi::{
        ClapLibrary, ClapPluginDescriptor, ClapPluginFactory, CLAP_PLUGIN_FACTORY_ID,
    };
    let (_lib, entry) = ClapLibrary::load(path).ok()?;
    unsafe {
        let factory =
            ((*entry).get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const std::ffi::c_char)
                as *const ClapPluginFactory;
        if factory.is_null() || ((*factory).get_plugin_count)(factory) == 0 {
            return None;
        }
        let desc: *const ClapPluginDescriptor = ((*factory).get_plugin_descriptor)(factory, 0);
        if desc.is_null() {
            return None;
        }
        Some(format!("clap:{}", (*desc).id_str()))
    }
}

#[test]
fn clap_real_instance_full_lifecycle() {
    let Some(path) = discover_all("ECHOES_TEST_CLAP", "clap").into_iter().next() else {
        eprintln!("{LOG} SKIP clap_real_instance_full_lifecycle — no .clap found");
        return;
    };
    let Some(device_id) = first_clap_id(&path) else {
        eprintln!("{LOG} SKIP — {} exposes no CLAP plugin", path.display());
        return;
    };
    eprintln!("{LOG} CLAP target: {device_id}  ({})", path.display());

    let mut host = PluginHost::new();
    host.registry_mut().add(clap_manifest(&device_id, &path));

    let key = "harness:clap:fx0";
    let node = host
        .instantiate(key, &device_id, false)
        .expect("real CLAP instantiation must succeed");
    let instance = host.instance_by_key(key).expect("instance registered");

    run_lifecycle(
        host,
        Subject {
            device_id,
            instance_key: key.to_string(),
            node,
            instance,
        },
        |inst, sr, block| inst.clap_ensure_active(sr, block),
    );
}

fn clap_manifest(device_id: &str, path: &Path) -> PluginManifest {
    PluginManifest {
        id: device_id.to_string(),
        name: "Harness CLAP".into(),
        vendor: String::new(),
        format: DeviceFormat::Clap,
        path: Some(path.to_path_buf()),
        categories: vec!["CLAP".into()],
        num_sidechain_inputs: 0,
        is_instrument: false,
    }
}

// ── VST3 ──────────────────────────────────────────────────────────────────────

/// First VST3 class id (`vst3:<hex>`) exported by a single-file `.vst3`.
fn first_vst3_id(path: &Path) -> Option<String> {
    use crate::plugin_formats::vst3::ffi::{enumerate_classes, Vst3Library};
    use crate::plugin_formats::vst3::host::hex_cid;
    let (_lib, factory) = Vst3Library::load(path).ok()?;
    let classes = unsafe { enumerate_classes(factory) };
    unsafe {
        ((*(*factory).vtbl).release)(factory);
    }
    classes
        .first()
        .map(|class| format!("vst3:{}", hex_cid(&class.cid)))
}

/// Every installed VST3 offering `IPluginFactory2` reports sub-categories, and an
/// instrument is told from an effect by them, not by its name.
#[test]
fn vst3_classes_report_their_sub_categories() {
    use crate::plugin_formats::vst3::ffi::{
        enumerate_classes, is_instrument_subcategory, Vst3Library,
    };

    let candidates = discover_all("ECHOES_TEST_VST3", "vst3");
    if candidates.is_empty() {
        eprintln!("{LOG} SKIP vst3_classes_report_their_sub_categories — no .vst3 found");
        return;
    }
    for path in &candidates {
        let Ok((_lib, factory)) = Vst3Library::load(path) else {
            continue;
        };
        let classes = unsafe { enumerate_classes(factory) };
        unsafe {
            ((*(*factory).vtbl).release)(factory);
        }
        for class in classes {
            let Some(sub_categories) = &class.sub_categories else {
                eprintln!("{LOG} {} — no IPluginFactory2", class.name);
                continue;
            };
            eprintln!(
                "{LOG} {} → \"{sub_categories}\" (instrument: {})",
                class.name,
                is_instrument_subcategory(sub_categories)
            );
            assert!(
                !sub_categories.is_empty(),
                "{} reports empty sub-categories",
                class.name
            );
        }
    }
}

/// Big-endian VST3 TUID → Windows COM byte order: reverse Data1 (`[0..4]`),
/// Data2 (`[4..6]`), Data3 (`[6..8]`); keep Data4 (`[8..16]`).
fn com_swap(mut iid: [u8; 16]) -> [u8; 16] {
    iid.swap(0, 3);
    iid.swap(1, 2);
    iid.swap(4, 5);
    iid.swap(6, 7);
    iid
}

/// Empirical root-cause proof for the VST3 `E_NOINTERFACE`: call the *same*
/// class's `createInstance` with the engine's big-endian `IID_ICOMPONENT`
/// (expected to fail) and then with the COM-swapped IID (expected S_OK).  If the
/// swapped IID succeeds, the failure is purely an interface-IID byte-order issue,
/// not a plugin-specific or dual-component problem.
fn vst3_iid_byteorder_probe(path: &Path) -> Option<String> {
    use std::ffi::c_void;

    use crate::plugin_formats::vst3::ffi::{
        enumerate_classes, IComponent, Vst3Library, IID_ICOMPONENT, K_RESULT_OK,
    };

    let (_lib, factory) = Vst3Library::load(path).ok()?;
    let cid = unsafe { enumerate_classes(factory) }.first()?.cid;
    let swapped = com_swap(IID_ICOMPONENT);

    unsafe {
        let mut a: *mut c_void = std::ptr::null_mut();
        let r_engine = ((*(*factory).vtbl).create_instance)(
            factory,
            cid.as_ptr(),
            IID_ICOMPONENT.as_ptr(),
            &mut a,
        );
        if r_engine == K_RESULT_OK && !a.is_null() {
            let c = a as *mut IComponent;
            ((*(*c).vtbl).release)(c);
            ((*(*factory).vtbl).release)(factory);
            return Some(format!(
                "inconclusive: engine IID already accepted (r={r_engine})"
            ));
        }

        let mut b: *mut c_void = std::ptr::null_mut();
        let r_swap =
            ((*(*factory).vtbl).create_instance)(factory, cid.as_ptr(), swapped.as_ptr(), &mut b);
        let swap_ok = r_swap == K_RESULT_OK && !b.is_null();
        if swap_ok {
            let c = b as *mut IComponent;
            ((*(*c).vtbl).release)(c);
        }
        ((*(*factory).vtbl).release)(factory);

        Some(format!(
            "engine big-endian IID → {r_engine} [{}];  COM-swapped IID → {r_swap} [{}]  ⇒ {}",
            hresult_name(r_engine),
            if swap_ok {
                "S_OK (component created)"
            } else {
                hresult_name(r_swap)
            },
            if swap_ok {
                "ROOT CAUSE CONFIRMED: interface IID byte order (engine uses non-COM order on Windows)"
            } else {
                "root cause unconfirmed (swapped IID also rejected)"
            },
        ))
    }
}

// Full real-VST3 lifecycle (T1–T5).  Unblocked once the engine's interface IIDs
// were corrected to COM byte order on Windows (finding F1); the byte-order probe
// in the all-failed branch remains as a regression explainer.
#[test]
fn vst3_real_instance_full_lifecycle() {
    let candidates = discover_all("ECHOES_TEST_VST3", "vst3");
    if candidates.is_empty() {
        eprintln!("{LOG} SKIP vst3_real_instance_full_lifecycle — no .vst3 found");
        return;
    }

    // Try each real VST3 until one instantiates, logging every divergence so a
    // plugin the engine cannot host (e.g. dual-component, audit V4) is surfaced
    // rather than hidden.  The lifecycle is validated on the first that loads.
    let key = "harness:vst3:fx0";
    let mut divergences = Vec::new();
    for path in &candidates {
        let Some(device_id) = first_vst3_id(path) else {
            eprintln!("{LOG} skip {} — no audio-module class", path.display());
            continue;
        };
        let mut host = PluginHost::new();
        host.registry_mut().add(vst3_manifest(&device_id, path));

        match host.instantiate(key, &device_id, false) {
            Ok(node) => {
                eprintln!("{LOG} VST3 target: {device_id}  ({})", path.display());
                let instance = host.instance_by_key(key).expect("instance registered");
                check_vst3_params(&host, key, &instance);
                run_lifecycle(
                    host,
                    Subject {
                        device_id,
                        instance_key: key.to_string(),
                        node,
                        instance,
                    },
                    |inst, sr, block| inst.vst3_ensure_active(sr, block),
                );
                if !divergences.is_empty() {
                    eprintln!("{LOG} VST3 DIVERGENCES (engine could not host these real plugins):");
                    for d in &divergences {
                        eprintln!("{LOG}   - {d}");
                    }
                }
                return;
            }
            Err(e) => {
                let msg = format!("{e}");
                let annotated = annotate_hresult(&msg);
                eprintln!(
                    "{LOG} DIVERGENCE: {} failed to instantiate — {annotated}",
                    path.display()
                );
                divergences.push(format!("{}: {annotated}", path.display()));
            }
        }
    }

    // Every real VST3 failed.  Probe the byte-order hypothesis on the first
    // candidate so the failure is explained, not just reported.
    if let Some(first) = candidates.first() {
        if let Some(proof) = vst3_iid_byteorder_probe(first) {
            eprintln!("{LOG} VST3 IID byte-order probe on {}:", first.display());
            eprintln!("{LOG}   {proof}");
        }
    }
    panic!(
        "no real VST3 could be instantiated by the engine ({} candidate(s) all failed): {:#?}",
        candidates.len(),
        divergences
    );
}

/// T0 (VST3) — the edit controller lives with the instance: parameters are
/// listed with their text, and a state round-trip keeps their values.
fn check_vst3_params(host: &PluginHost, key: &str, instance: &PluginInstance) {
    if instance.vst3_controller().is_none() {
        eprintln!("{LOG} T0 SKIP — this VST3 offers no edit controller");
        return;
    }
    let params = host
        .plugin_params(key)
        .expect("T0: parameters must be readable");
    eprintln!("{LOG} T0 {} parameter(s)", params.len());
    let Some(param) = params.iter().find(|param| param.automatable) else {
        eprintln!("{LOG} T0 no automatable parameter — text check skipped");
        return;
    };
    assert_eq!(
        (param.min, param.max),
        (0.0, 1.0),
        "T0: VST3 ranges are normalised"
    );
    let text = host.plugin_param_text(key, param.native_id, param.default);
    eprintln!(
        "{LOG} T0 '{}' default {} → {text:?}",
        param.name, param.default
    );
    assert!(
        text.is_some(),
        "T0: the plugin must render its own parameter text"
    );

    let before = instance.param_value(param.native_id);
    let Some(state) = instance.save_state_blob() else {
        eprintln!("{LOG} T0 the plugin reports no state — round-trip skipped");
        return;
    };
    assert!(
        instance.load_state_blob(&state),
        "T0: VST3 state must reload"
    );
    assert_eq!(
        instance.param_value(param.native_id),
        before,
        "T0: a state round-trip keeps the controller's value"
    );
}

/// Append the COM symbol for a trailing HRESULT in an instantiation error.
fn annotate_hresult(msg: &str) -> String {
    if let Some(tok) = msg.split_whitespace().last() {
        if let Ok(code) = tok.parse::<i32>() {
            let name = hresult_name(code);
            if name != "?" {
                return format!("{msg}  [{name}]");
            }
        }
    }
    msg.to_string()
}

fn vst3_manifest(device_id: &str, path: &Path) -> PluginManifest {
    PluginManifest {
        id: device_id.to_string(),
        name: "Harness VST3".into(),
        vendor: String::new(),
        format: DeviceFormat::Vst3,
        path: Some(path.to_path_buf()),
        categories: vec!["VST3".into()],
        num_sidechain_inputs: 0,
        is_instrument: false,
    }
}

/// A VST3 instrument found on disk: its manifest, from the factory's
/// sub-categories rather than its name.
fn first_vst3_instrument(candidates: &[PathBuf]) -> Option<PluginManifest> {
    use crate::plugin_formats::vst3::ffi::{
        enumerate_classes, is_instrument_subcategory, Vst3Library,
    };
    use crate::plugin_formats::vst3::host::hex_cid;

    candidates.iter().find_map(|path| {
        let (_lib, factory) = Vst3Library::load(path).ok()?;
        let classes = unsafe { enumerate_classes(factory) };
        unsafe {
            ((*(*factory).vtbl).release)(factory);
        }
        let class = classes.into_iter().find(|class| {
            class
                .sub_categories
                .as_deref()
                .is_some_and(is_instrument_subcategory)
        })?;
        let mut manifest = vst3_manifest(&format!("vst3:{}", hex_cid(&class.cid)), path);
        manifest.name = class.name;
        manifest.is_instrument = true;
        Some(manifest)
    })
}

/// A real VST3 instrument turns a note into sound and goes quiet once the
/// note is released and the adapter is reset.
#[test]
fn vst3_instrument_plays_a_note() {
    use crate::audio_graph::midi::{MidiEvent, MidiEventKind, MidiEventList};

    let candidates = discover_all("ECHOES_TEST_VST3", "vst3");
    let Some(manifest) = first_vst3_instrument(&candidates) else {
        eprintln!("{LOG} SKIP vst3_instrument_plays_a_note — no VST3 instrument found");
        return;
    };
    eprintln!("{LOG} VST3 instrument: {} ({})", manifest.name, manifest.id);

    let key = "harness:vst3:instrument0";
    let mut host = PluginHost::new();
    host.registry_mut().add(manifest.clone());
    let mut device = host
        .instantiate_instrument(key, &manifest.id)
        .expect("a VST3 instrument must instantiate");

    let frames = 256;
    device.prepare(PrepareContext {
        sample_rate: 48_000.0,
        max_block_size: 512,
        num_channels: 2,
        live: true,
    });
    let transport = TransportInfo {
        is_playing: true,
        position_samples: 0,
        position_seconds: 0.0,
        bpm: 120.0,
        time_signature: crate::audio_graph::node::TimeSignature::default(),
        sample_rate: 48_000.0,
        block_size: frames,
    };
    let render = |device: &mut Box<dyn crate::audio_graph::nodes::instrument::InstrumentDevice>,
                  events: &MidiEventList|
     -> f32 {
        let mut outputs = vec![AudioBuffer::new(2, frames)];
        device.process_midi(events, &mut outputs, &transport);
        let mut peak = 0.0f32;
        for channel in 0..2 {
            for &sample in outputs[0].channel(channel).iter().take(frames) {
                assert!(sample.is_finite(), "output must be finite");
                peak = peak.max(sample.abs());
            }
        }
        peak
    };

    let mut note_on = MidiEventList::new();
    note_on.push(MidiEvent {
        offset: 0,
        kind: MidiEventKind::NoteOn {
            pitch: 60,
            velocity: 100,
        },
    });
    let silence = MidiEventList::new();

    let mut peak = render(&mut device, &note_on);
    for _ in 0..32 {
        peak = peak.max(render(&mut device, &silence));
    }
    eprintln!("{LOG} instrument peak after a note: {peak}");
    assert!(peak > 0.0, "a played note must produce a non-silent signal");

    let mut note_off = MidiEventList::new();
    note_off.push(MidiEvent {
        offset: 0,
        kind: MidiEventKind::NoteOff { pitch: 60 },
    });
    render(&mut device, &note_off);
    device.reset();
    for _ in 0..8 {
        render(&mut device, &silence);
    }
}

/// A real VST3 instrument tells which parameters its MIDI controllers drive;
/// the host reads the table without crashing the plugin.
#[test]
fn vst3_instrument_reports_its_midi_controller_mapping() {
    use crate::plugin_formats::vst3::midi_mapping::{
        read_midi_mapping, CTRL_AFTER_TOUCH, CTRL_PITCH_BEND,
    };

    let candidates = discover_all("ECHOES_TEST_VST3", "vst3");
    let Some(manifest) = first_vst3_instrument(&candidates) else {
        eprintln!("{LOG} SKIP vst3_instrument_reports_its_midi_controller_mapping — no VST3 instrument found");
        return;
    };
    let key = "harness:vst3:instrument1";
    let mut host = PluginHost::new();
    host.registry_mut().add(manifest.clone());
    let _device = host
        .instantiate_instrument(key, &manifest.id)
        .expect("a VST3 instrument must instantiate");
    let instance = host.instance_by_key(key).expect("instance registered");

    let Some(controller) = instance.vst3_controller() else {
        eprintln!("{LOG} {} has no edit controller", manifest.name);
        return;
    };
    let _gate = instance
        .offrt_try_acquire(crate::plugin_system::instance::FFI_GATE_HARVEST_BUDGET)
        .expect("the FFI gate is free");
    let map = unsafe { read_midi_mapping(controller.as_ptr()) };
    eprintln!(
        "{LOG} {} maps pitch bend → {:?}, mod wheel → {:?}, aftertouch → {:?}",
        manifest.name,
        map.param_for(CTRL_PITCH_BEND),
        map.param_for(1),
        map.param_for(CTRL_AFTER_TOUCH),
    );
}

/// Step 5 — a real controller is told its track, and the host creates messages
/// for it: nothing crashes, and a controller that offers `IInfoListener` accepts
/// the list.  What the plugin does with the name is not observable from here.
#[test]
fn vst3_real_controller_takes_host_messages_and_track_info() {
    use crate::plugin_system::track_info::test_track;
    use crate::project::AppProject;

    let candidates = discover_all("ECHOES_TEST_VST3", "vst3");
    if candidates.is_empty() {
        eprintln!(
            "{LOG} SKIP vst3_real_controller_takes_host_messages_and_track_info — no .vst3 found"
        );
        return;
    }

    let key = "harness:vst3:track-info";
    for path in &candidates {
        let Some(device_id) = first_vst3_id(path) else {
            continue;
        };
        let mut host = PluginHost::new();
        host.registry_mut().add(vst3_manifest(&device_id, path));
        let mut project = AppProject::new();
        project.tracks = vec![test_track("t1", "Lead", Some("#102030"), &[key])];
        host.sync_track_info(&project);
        let Ok(_node) = host.instantiate(key, &device_id, false) else {
            continue;
        };
        let instance = host.instance_by_key(key).expect("instance registered");
        eprintln!(
            "{LOG} VST3 track-info target: {device_id}  ({})",
            path.display()
        );

        check_vst3_track_info(&mut host, &mut project, &instance);
        check_vst3_host_messages(&instance);
        return;
    }
    panic!("no real VST3 could be instantiated by the engine");
}

/// Step 6 — what a real controller says about `IUnitInfo`: whether it offers the
/// interface, and its program lists.  Read-only; the findings are logged, only
/// consistency is asserted.
#[test]
fn vst3_real_controller_program_lists() {
    use crate::plugin_formats::vst3::ffi::{FUnknown, IID_IUNIT_INFO};
    use crate::plugin_formats::vst3::units::read_program_lists;

    let candidates = discover_all("ECHOES_TEST_VST3", "vst3");
    if candidates.is_empty() {
        eprintln!("{LOG} SKIP vst3_real_controller_program_lists — no .vst3 found");
        return;
    }

    let key = "harness:vst3:units";
    for path in &candidates {
        let Some(device_id) = first_vst3_id(path) else {
            continue;
        };
        let mut host = PluginHost::new();
        host.registry_mut().add(vst3_manifest(&device_id, path));
        let Ok(_node) = host.instantiate(key, &device_id, false) else {
            continue;
        };
        let instance = host.instance_by_key(key).expect("instance registered");
        let Some(controller) = instance.vst3_controller() else {
            eprintln!("{LOG} U0 SKIP — {device_id} offers no edit controller");
            continue;
        };
        let _gate = instance
            .offrt_try_acquire(std::time::Duration::from_millis(50))
            .expect("U0: an idle instance gives its FFI gate");
        let offered = unsafe { offers(controller.as_ptr() as *mut FUnknown, &IID_IUNIT_INFO) };
        let lists = unsafe { read_program_lists(controller.as_ptr()) };
        eprintln!(
            "{LOG} U0 {device_id} ({}): IUnitInfo offered: {offered}, {} program list(s)",
            path.display(),
            lists.len()
        );
        for list in &lists {
            eprintln!(
                "{LOG} U0   list {} '{}': {} program(s), selector {:?}, first: {:?}",
                list.id,
                list.name,
                list.programs.len(),
                list.program_change_param,
                list.programs.iter().take(3).collect::<Vec<_>>()
            );
        }
        unsafe { log_units_and_program_change_params(controller.as_ptr()) };
        assert!(
            offered || lists.is_empty(),
            "U0: lists come only from a controller that offers IUnitInfo"
        );
        let mut ids: Vec<_> = lists.iter().map(|list| list.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), lists.len(), "U0: program list ids are unique");
        assert!(
            !instance.has_crashed(),
            "U0: reading must not crash the plugin"
        );
        return;
    }
    panic!("no real VST3 could be instantiated by the engine");
}

/// Step 6 — a `.vstpreset` file made from a real plugin's own state is listed under
/// the standard folders, and loading it brings that state back, whatever the
/// plugin was set to meanwhile.  No such file exists on this machine, so the file
/// is made here; what is checked is the host's side of the format.
#[test]
fn vst3_real_preset_file_is_listed_and_loaded() {
    use crate::plugin_formats::vst3::host::decode_plugin_id;
    use crate::plugin_formats::vst3::preset_file::{class_id_text, tests::preset_file};
    use crate::plugin_formats::vst3::preset_locations::PresetRoots;
    use crate::plugin_system::presets::{PresetListing, Vst3PresetFiles};

    let candidates = discover_all("ECHOES_TEST_VST3", "vst3");
    if candidates.is_empty() {
        eprintln!("{LOG} SKIP vst3_real_preset_file_is_listed_and_loaded — no .vst3 found");
        return;
    }

    let key = "harness:vst3:preset-file";
    for path in &candidates {
        let Some(device_id) = first_vst3_id(path) else {
            continue;
        };
        let mut host = PluginHost::new();
        host.registry_mut().add(vst3_manifest(&device_id, path));
        let Ok(mut node) = host.instantiate(key, &device_id, false) else {
            continue;
        };
        let instance = host.instance_by_key(key).expect("instance registered");
        assert!(instance.vst3_ensure_active(48_000.0, 512));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 512,
            num_channels: 2,
            live: true,
        });
        let programs = instance.vst3_program_presets();
        if programs.len() < 3 {
            eprintln!("{LOG} F0 {device_id}: too few programs to tell states apart");
            continue;
        }

        let directory =
            std::env::temp_dir().join(format!("echoes-real-vstpreset-{}", std::process::id()));
        let folder = directory.join("docs/VST3 Presets/Harness/Harness VST3");
        std::fs::create_dir_all(&folder).unwrap();
        let class_id = class_id_text(&decode_plugin_id(&device_id).unwrap());
        let listing = PresetListing::Vst3 {
            instance: Arc::clone(&instance),
            files: Some(
                Vst3PresetFiles::new("Harness VST3", &device_id)
                    .unwrap()
                    .under(PresetRoots {
                        documents: Some(directory.join("docs")),
                        ..PresetRoots::default()
                    }),
            ),
        };

        let running = std::sync::atomic::AtomicBool::new(true);
        let frames = 256;
        let outcome = std::thread::scope(|scope| {
            let audio = scope.spawn(|| {
                let input = AudioBuffer::new(2, frames);
                let transport = TransportInfo {
                    block_size: frames,
                    ..TransportInfo::stopped(48_000.0)
                };
                while running.load(std::sync::atomic::Ordering::Acquire) {
                    let mut outputs = vec![AudioBuffer::new(2, frames)];
                    node.process(&[&input], &mut outputs, &transport);
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            });
            let choose = |index: usize| {
                let request = programs[index].request();
                instance.load_preset(&request).expect("a program loads");
            };
            let values = || -> Vec<f64> {
                host.plugin_params(key)
                    .expect("parameters are readable")
                    .iter()
                    .map(|param| instance.param_value(param.native_id).unwrap_or(f64::NAN))
                    .collect()
            };
            choose(2);
            let saved_values = values();
            let saved_state = instance.save_state_blob().expect("the plugin has a state");
            std::fs::write(
                folder.join("Mine.vstpreset"),
                preset_file(&class_id, &[(b"Comp", &saved_state)]),
            )
            .unwrap();
            choose(5);
            let other_values = values();

            let listed = listing.list();
            let file = listed
                .iter()
                .find(|preset| preset.name == "Mine")
                .expect("the file is listed")
                .clone();
            let edits_before = instance.state_edit_count();
            let loaded = instance.load_preset(&file.request());
            let values_after = values();
            running.store(false, std::sync::atomic::Ordering::Release);
            audio.join().expect("the audio thread must not crash");
            (
                file,
                loaded,
                saved_state.len(),
                (saved_values, other_values, values_after),
                edits_before,
            )
        });
        let (file, loaded, state_length, (saved, other, after), edits_before) = outcome;
        eprintln!(
            "{LOG} F0 {device_id}: listed '{}' ({} bytes of state) → {loaded:?}",
            file.name, state_length
        );
        loaded.expect("F0: a file the listing offered must load");
        assert_ne!(saved, other, "F0: the two programs differ");
        assert_eq!(after, saved, "F0: the file brought the parameters back");
        assert_eq!(
            instance.state_edit_count(),
            edits_before + 1,
            "F0: marked unsaved"
        );
        assert!(
            !instance.has_crashed(),
            "F0: loading a file must not crash the plugin"
        );
        let _ = std::fs::remove_dir_all(directory);
        return;
    }
    eprintln!("{LOG} SKIP vst3_real_preset_file_is_listed_and_loaded — no VST3 with programs");
}

/// Step 6 — a program a real plugin lists can be chosen: the selector moves, the
/// other parameters follow when the plugin's controller shares its processor's
/// state (what the log shows), the project reads as unsaved, nothing crashes.
#[test]
fn vst3_real_program_can_be_chosen() {
    let candidates = discover_all("ECHOES_TEST_VST3", "vst3");
    if candidates.is_empty() {
        eprintln!("{LOG} SKIP vst3_real_program_can_be_chosen — no .vst3 found");
        return;
    }

    let key = "harness:vst3:program";
    for path in &candidates {
        let Some(device_id) = first_vst3_id(path) else {
            continue;
        };
        let mut host = PluginHost::new();
        host.registry_mut().add(vst3_manifest(&device_id, path));
        let Ok(mut node) = host.instantiate(key, &device_id, false) else {
            continue;
        };
        let instance = host.instance_by_key(key).expect("instance registered");
        assert!(instance.vst3_ensure_active(48_000.0, 512));
        node.prepare(PrepareContext {
            sample_rate: 48_000.0,
            max_block_size: 512,
            num_channels: 2,
            live: true,
        });
        let programs = instance.vst3_program_presets();
        if programs.len() < 2 {
            eprintln!(
                "{LOG} P0 {device_id}: {} program(s), nothing to choose",
                programs.len()
            );
            continue;
        }
        let listing = host.preset_listing(key).expect("a VST3 placement lists");
        host.remember_listing(&listing, programs.clone());

        let values_of = |instance: &PluginInstance| -> Vec<f64> {
            host.plugin_params(key)
                .expect("parameters are readable")
                .iter()
                .map(|param| instance.param_value(param.native_id).unwrap_or(f64::NAN))
                .collect()
        };
        let before = values_of(&instance);
        let state_before = instance.save_state_blob();
        let edits_before = instance.state_edit_count();

        // The audio thread runs while the program is chosen, as in the app.
        let running = std::sync::atomic::AtomicBool::new(true);
        let frames = 256;
        let chosen = &programs[2];
        let request = chosen.request();
        let loaded = std::thread::scope(|scope| {
            let audio = scope.spawn(|| {
                let input = AudioBuffer::new(2, frames);
                let transport = TransportInfo {
                    block_size: frames,
                    ..TransportInfo::stopped(48_000.0)
                };
                while running.load(std::sync::atomic::Ordering::Acquire) {
                    let mut outputs = vec![AudioBuffer::new(2, frames)];
                    node.process(&[&input], &mut outputs, &transport);
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            });
            let loaded = host
                .instance_for_preset(key, &request)
                .and_then(|instance| instance.load_preset(&request));
            running.store(false, std::sync::atomic::Ordering::Release);
            audio.join().expect("the audio thread must not crash");
            loaded
        });
        eprintln!(
            "{LOG} P0 {device_id}: chose '{}' of {} → {loaded:?}",
            chosen.name,
            programs.len()
        );
        loaded.expect("P0: a program the plugin listed must load");

        let active = |instance: &PluginInstance| -> Vec<String> {
            instance
                .vst3_program_presets()
                .into_iter()
                .filter(|preset| preset.active)
                .map(|preset| preset.name)
                .collect()
        };
        eprintln!("{LOG} P0   active program now: {:?}", active(&instance));
        assert_eq!(
            active(&instance),
            std::slice::from_ref(&chosen.name),
            "P0: the plugin names the program that was chosen"
        );

        let after = values_of(&instance);
        let changed = before.iter().zip(&after).filter(|(a, b)| a != b).count();
        let state_changed = state_before != instance.save_state_blob();
        eprintln!(
            "{LOG} P0   {changed} of {} parameter value(s) changed, state changed: {state_changed}",
            before.len()
        );
        assert!(
            changed > 1,
            "P0: the knobs must follow the program, not only its selector"
        );
        assert!(state_changed, "P0: the saved state holds the program");
        assert_eq!(
            instance.state_edit_count(),
            edits_before + 1,
            "P0: marked unsaved"
        );
        assert!(
            instance
                .offrt_try_acquire(std::time::Duration::ZERO)
                .is_some(),
            "P0: gate freed"
        );
        assert!(
            !instance.has_crashed(),
            "P0: choosing a program must not crash the plugin"
        );
        return;
    }
    eprintln!("{LOG} SKIP vst3_real_program_can_be_chosen — no VST3 with several programs");
}

/// Logs the units of a controller and every parameter flagged `kIsProgramChange`,
/// to see why a program list has no selector.
unsafe fn log_units_and_program_change_params(
    controller: *mut crate::plugin_formats::vst3::ffi::IEditController,
) {
    use crate::plugin_formats::vst3::ffi::{
        string128_to_string, ParameterInfo, IID_IUNIT_INFO, K_RESULT_OK, PARAM_IS_PROGRAM_CHANGE,
    };
    use crate::plugin_formats::vst3::units::{IUnitInfoVtbl, UnitInfo};

    let vtbl = &*(*controller).vtbl;
    let count = (vtbl.get_parameter_count)(controller);
    let mut flagged = 0;
    for index in 0..count {
        let mut info: ParameterInfo = std::mem::zeroed();
        if (vtbl.get_parameter_info)(controller, index, &mut info) != K_RESULT_OK {
            continue;
        }
        if info.flags & PARAM_IS_PROGRAM_CHANGE != 0 {
            flagged += 1;
            eprintln!(
                "{LOG} U0   program-change parameter {} '{}' unit {} steps {}",
                info.id,
                string128_to_string(&info.title),
                info.unit_id,
                info.step_count
            );
        }
    }
    eprintln!("{LOG} U0   {count} parameter(s), {flagged} flagged kIsProgramChange");

    let mut interface: *mut std::ffi::c_void = std::ptr::null_mut();
    if (vtbl.query_interface)(controller, &IID_IUNIT_INFO, &mut interface) != K_RESULT_OK {
        return;
    }
    let units = &**(interface as *mut *const IUnitInfoVtbl);
    for index in 0..(units.get_unit_count)(interface) {
        let mut info: UnitInfo = std::mem::zeroed();
        if (units.get_unit_info)(interface, index, &mut info) == K_RESULT_OK {
            eprintln!(
                "{LOG} U0   unit {} '{}' parent {} list {}",
                info.id,
                string128_to_string(&info.name),
                info.parent_unit_id,
                info.program_list_id
            );
        }
    }
    (units.release)(interface);
}

/// QI of `object` for `iid`, giving the reference straight back.
unsafe fn offers(
    object: *mut crate::plugin_formats::vst3::ffi::FUnknown,
    iid: &crate::plugin_formats::vst3::ffi::Tuid,
) -> bool {
    let mut found: *mut std::ffi::c_void = std::ptr::null_mut();
    let result = ((*(*object).vtbl).query_interface)(object, iid, &mut found);
    if result != 0 || found.is_null() {
        return false;
    }
    let found = found as *mut crate::plugin_formats::vst3::ffi::FUnknown;
    ((*(*found).vtbl).release)(found);
    true
}

fn check_vst3_track_info(
    host: &mut PluginHost,
    project: &mut crate::project::AppProject,
    instance: &PluginInstance,
) {
    use crate::plugin_formats::clap::ffi::HostTrackInfo;
    use crate::plugin_formats::vst3::channel_context::notify_channel_context;
    use crate::plugin_formats::vst3::ffi::{FUnknown, IID_IINFO_LISTENER};

    let Some(controller) = instance.vst3_controller() else {
        eprintln!("{LOG} T5 SKIP — this VST3 offers no edit controller");
        return;
    };
    let object = controller.as_ptr() as *mut FUnknown;
    let listens = unsafe { offers(object, &IID_IINFO_LISTENER) };
    eprintln!("{LOG} T5 controller implements IInfoListener: {listens}");

    {
        let _gate = instance
            .offrt_try_acquire(std::time::Duration::from_millis(50))
            .expect("T5: an idle instance gives its FFI gate");
        let info = HostTrackInfo::new("Lead", Some([0x10, 0x20, 0x30]));
        let told = unsafe { notify_channel_context(object, &info) };
        assert_eq!(
            told, listens,
            "T5: a controller with IInfoListener must accept the list"
        );
    }

    // What the app does on a rename and a recolor: no recompile, no crash.
    project.tracks[0].name = "Pad".into();
    project.tracks[0].color = None;
    host.sync_track_info(project);
    host.notify_pending_track_info();
    assert!(
        !instance.has_crashed(),
        "T5: telling the plugin its track must not crash it"
    );
}

fn check_vst3_host_messages(instance: &PluginInstance) {
    use crate::plugin_formats::vst3::ffi::{
        FUnknown, IAttributeListVtbl, IConnectionPoint, IHostApplicationVtbl, IMessageVtbl,
        IID_IATTRIBUTE_LIST, IID_IBSTREAM, IID_ICONNECTION_POINT, IID_IMESSAGE, K_RESULT_FALSE,
        K_RESULT_OK,
    };
    use crate::plugin_formats::vst3::host_context::host_application;
    use std::ffi::{c_void, CString};

    let host_app = host_application();
    let create = unsafe { (*(*(host_app as *const *const IHostApplicationVtbl))).create_instance };

    let mut refused: *mut c_void = std::ptr::null_mut();
    let result = unsafe { create(host_app, &IID_IBSTREAM, &IID_IBSTREAM, &mut refused) };
    assert_eq!(
        result, K_RESULT_FALSE,
        "T5: an unknown interface is refused"
    );
    assert!(refused.is_null());

    let mut list: *mut c_void = std::ptr::null_mut();
    let result = unsafe {
        create(
            host_app,
            &IID_IATTRIBUTE_LIST,
            &IID_IATTRIBUTE_LIST,
            &mut list,
        )
    };
    assert_eq!(result, K_RESULT_OK, "T5: the host makes an IAttributeList");
    let list = list as *mut FUnknown;
    unsafe { ((*(*list).vtbl).release)(list) };

    let mut raw: *mut c_void = std::ptr::null_mut();
    let result = unsafe { create(host_app, &IID_IMESSAGE, &IID_IMESSAGE, &mut raw) };
    assert_eq!(result, K_RESULT_OK, "T5: the host makes an IMessage");
    let message = raw as *mut FUnknown;
    unsafe {
        let vtbl = &*((*message).vtbl as *const IMessageVtbl);
        (vtbl.set_message_id)(message, CString::new("EchoesHarness").unwrap().as_ptr());
        let attributes = (vtbl.get_attributes)(message);
        let attributes_vtbl = &*((*attributes).vtbl as *const IAttributeListVtbl);
        (attributes_vtbl.set_int)(attributes, CString::new("n").unwrap().as_ptr(), 1);
    }

    // As a plugin half would: through the connection points, which the host
    // wired together; a message the plugin does not know must be ignored.
    let Some((component, _)) = instance.vst3_handles() else {
        return;
    };
    let halves = [
        ("component", component.as_ptr() as *mut FUnknown),
        (
            "controller",
            instance
                .vst3_controller()
                .map_or(std::ptr::null_mut(), |c| c.as_ptr() as *mut FUnknown),
        ),
    ];
    {
        let _gate = instance
            .offrt_try_acquire(std::time::Duration::from_millis(50))
            .expect("T5: an idle instance gives its FFI gate");
        for (name, half) in halves {
            if half.is_null() {
                continue;
            }
            let mut found: *mut c_void = std::ptr::null_mut();
            let queried = unsafe {
                ((*(*half).vtbl).query_interface)(half, &IID_ICONNECTION_POINT, &mut found)
            };
            if queried != K_RESULT_OK || found.is_null() {
                eprintln!("{LOG} T5 {name} has no IConnectionPoint");
                continue;
            }
            let point = found as *mut IConnectionPoint;
            let answer = unsafe { ((*(*point).vtbl).notify)(point, message as *mut c_void) };
            eprintln!("{LOG} T5 {name} answered the message with {answer:#x}");
            unsafe { ((*(*point).vtbl).release)(point) };
        }
    }
    let remaining = unsafe { ((*(*message).vtbl).release)(message) };
    eprintln!("{LOG} T5 message references left after release: {remaining}");
}
