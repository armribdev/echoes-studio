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
use crate::audio_graph::node::{NodeId, PrepareContext, TransportInfo};
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
            .map_or(false, |e| e.eq_ignore_ascii_case(ext))
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
    eprintln!(
        "{LOG} CLAP target: {device_id}  ({})",
        path.display()
    );

    let mut host = PluginHost::new();
    host.registry_mut().add(clap_manifest(&device_id, &path));

    let key = "harness:clap:fx0";
    let node = host
        .instantiate(NodeId(1), key, &device_id, false)
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
        version: String::new(),
        format: DeviceFormat::Clap,
        path: Some(path.to_path_buf()),
        categories: vec!["CLAP".into()],
        num_audio_inputs: 2,
        num_audio_outputs: 2,
        has_midi_input: false,
        has_midi_output: false,
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
    classes.first().map(|(cid, _)| format!("vst3:{}", hex_cid(cid)))
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
    let (cid, _) = unsafe { enumerate_classes(factory) }.first()?.clone();
    let swapped = com_swap(IID_ICOMPONENT);

    unsafe {
        let mut a: *mut c_void = std::ptr::null_mut();
        let r_engine =
            ((*(*factory).vtbl).create_instance)(factory, cid.as_ptr(), IID_ICOMPONENT.as_ptr(), &mut a);
        if r_engine == K_RESULT_OK && !a.is_null() {
            let c = a as *mut IComponent;
            ((*(*c).vtbl).release)(c);
            ((*(*factory).vtbl).release)(factory);
            return Some(format!("inconclusive: engine IID already accepted (r={r_engine})"));
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
            if swap_ok { "S_OK (component created)" } else { hresult_name(r_swap) },
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

        match host.instantiate(NodeId(1), key, &device_id, false) {
            Ok(node) => {
                eprintln!("{LOG} VST3 target: {device_id}  ({})", path.display());
                let instance = host.instance_by_key(key).expect("instance registered");
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
        version: String::new(),
        format: DeviceFormat::Vst3,
        path: Some(path.to_path_buf()),
        categories: vec!["VST3".into()],
        num_audio_inputs: 2,
        num_audio_outputs: 2,
        has_midi_input: false,
        has_midi_output: false,
        num_sidechain_inputs: 0,
        is_instrument: false,
    }
}
