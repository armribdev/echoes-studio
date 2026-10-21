# FFI Plugin Lifecycle Validation (real instances)

**Date:** 2026-06-04 · **Scope:** real VST3 / CLAP instance lifecycle on Windows.
**Harness:** [`src-tauri/src/plugin_system/ffi_lifecycle_tests.rs`](../../../src-tauri/src/plugin_system/ffi_lifecycle_tests.rs)
(test-only; no production code, graph, scheduler, or plugin-system design was changed).

This validation closes the two gaps left by the existing suite, which only
exercised the **internal / pass-through bypass** path:

- **Lacune A** — real activation runtime (`init/activate/start_processing`,
  sample-rate / block-size re-init) was never exercised against a real plugin.
- **Lacune C** — real native teardown (`stop_processing → deactivate → destroy`
  / `setProcessing(0) → setActive(0) → terminate → release`) was never observed.

## How to run

```sh
# Real-cycle logs (CLAP runs; VST3 is #[ignore] — see below):
cargo test -p echoes-studio ffi_lifecycle -- --nocapture --test-threads=1

# Reproduce the VST3 finding + byte-order probe:
cargo test -p echoes-studio vst3_real_instance_full_lifecycle -- --ignored --nocapture
```

Discovery is gated on a real plugin being present, so a machine without plugins
logs `SKIP` and the suite stays green. Override with `ECHOES_TEST_CLAP=<path.clap>`
/ `ECHOES_TEST_VST3=<path.vst3>`.

Validated against: **Surge XT Effects** (CLAP, `org.surge-synth-team.surge-xt-fx`)
and **TDR Nova** (VST3) on Windows 11.

## Mandatory tests

|     | Test                                               | CLAP (Surge XT Effects)                                      | VST3 (TDR Nova)                                              |
| --- | -------------------------------------------------- | ------------------------------------------------------------ | ------------------------------------------------------------ |
| T1  | Activation `Created → activate → start_processing` | ✅ `active=true`                                             | ✅ `active=true` (after F1 fix)                              |
| T2  | Real audio block through `process` vtable          | ✅ output finite                                             | ✅ output finite                                             |
| T3  | Sample-rate reinit (48k→44.1k)                     | ✅ reactivated                                               | ✅ reactivated                                               |
| T4  | Block-size change (512→256)                        | ✅ re-init                                                   | ✅ re-init                                                   |
| T5  | Teardown / no leak                                 | ✅ `strong_count=1`, `Destroyed`, real FFI release, no crash | ✅ `strong_count=1`, `Destroyed`, real FFI release, no crash |

### CLAP — real cycle log

```text
[ffi-harness] CLAP target: clap:org.surge-synth-team.surge-xt-fx  (…\Surge XT Effects.clap)
[ffi-harness] T1 activation @48k/512: active=true
[ffi-harness] T2 processed 256 frames: output finite=true
[ffi-harness] T3 sample-rate reinit @44.1k/512: active=true
[ffi-harness] T4 block-size reinit @44.1k/256: active=true
[ffi-harness] T5 teardown: strong_count=1, lifecycle=Destroyed, key=harness:clap:fx0 — dropping (real FFI release)
[ffi-harness] T5 teardown complete — native handle released, no crash
```

**CLAP verdict:** the real activation runtime (`PluginInstance::clap_ensure_active`)
and native teardown (`PluginInstance::Drop`) are correct, including idempotent
re-activation on sample-rate and block-size changes. Lacune A and Lacune C are
**validated for CLAP**.

## Finding F1 — VST3 could not be instantiated on Windows (interface IID byte order) — FIXED

> **Status: fixed and verified.** The VST3 lifecycle test (T1–T5) now passes
> against a real plugin. The original failure and root cause are kept below for
> the record.

Before the fix, every real VST3 failed at instantiation:

```text
[ffi-harness] DIVERGENCE: …\TDR Nova.vst3 failed to instantiate
              — createInstance(IComponent) failed: -2147467262  [E_NOINTERFACE]
[ffi-harness] VST3 IID byte-order probe on …\TDR Nova.vst3:
[ffi-harness]   engine big-endian IID → -2147467262 [E_NOINTERFACE];
                COM-swapped IID → 0 [S_OK (component created)]
              ⇒ ROOT CAUSE CONFIRMED: interface IID byte order (engine uses non-COM order on Windows)
```

### Root cause (empirically proven)

VST3 on Windows is **COM-compatible**: interface identifiers (`Tuid`) use the
GUID byte layout (`Data1`/`Data2`/`Data3` little-endian, `Data4` big-endian).
The engine hardcodes its interface IIDs in **platform-neutral big-endian** order
in [`plugin_formats/vst3/ffi.rs`](../../../src-tauri/src/plugin_formats/vst3/ffi.rs):

```rust
// IComponent::iid = INLINE_UID(0xE831FF31, 0xF2D54301, 0x928EBBEE, 0x25697802)
pub const IID_ICOMPONENT: Tuid =
    [0xE8,0x31,0xFF,0x31, 0xF2,0xD5,0x43,0x01, 0x92,0x8E,0xBB,0xEE, 0x25,0x69,0x78,0x02]; // big-endian
// Correct Windows/COM order:
//  [0x31,0xFF,0x31,0xE8, 0xD5,0xF2,0x01,0x43, 0x92,0x8E,0xBB,0xEE, 0x25,0x69,0x78,0x02]
```

- The **class CID** round-trips fine: it is read from `getClassInfo` (already in
  the plugin's COM order) and passed straight back to `createInstance`, unchanged.
- The **interface IID** is the engine's own constant. In big-endian order the
  plugin's class does not recognise it → `E_NOINTERFACE`. The harness probe calls
  the _same_ class with the COM-swapped IID and gets `S_OK`, isolating byte order
  as the sole cause.

### Why this was never caught

- `enumerate_classes` / scan uses `getClassInfo`, which needs **no** interface
  IID, so VST3 plugins scan and register fine — only instantiation fails.
- The existing tests instantiate `echoes.gain` (internal `PassThroughDevice`),
  which never crosses the VST3 FFI boundary. No test had ever driven a real VST3
  through `createInstance` — exactly Lacune A.

### Impact (pre-fix)

- VST3 hosting was non-functional on Windows: no third-party VST3 could be
  instantiated, activated, processed, or torn down.
- CLAP was unaffected (CLAP is a pure C ABI with no COM byte-order rule).

### Fix applied

Minimal, scoped to the two interface IIDs on the instantiation/activation path,
in [`plugin_formats/vst3/ffi.rs`](../../../src-tauri/src/plugin_formats/vst3/ffi.rs):

```rust
const fn interface_iid(be: Tuid) -> Tuid {
    if cfg!(windows) {
        // GUID byte order: reverse Data1[0..4], Data2[4..6], Data3[6..8]; keep Data4.
        [be[3],be[2],be[1],be[0], be[5],be[4], be[7],be[6], be[8],be[9],be[10],be[11],be[12],be[13],be[14],be[15]]
    } else { be }
}
pub const IID_ICOMPONENT: Tuid = interface_iid([/* big-endian literal */]);
pub const IID_IAUDIO_PROCESSOR: Tuid = interface_iid([/* big-endian literal */]);
```

- The literals stay in readable big-endian form; the swap is `cfg(windows)`-only,
  so non-Windows behaviour is unchanged.
- **Class CIDs are not swapped** — they arrive from `getClassInfo` already in COM
  order and round-trip unchanged.
- **Scope deliberately limited** (per "touch only what is necessary"): only the
  two IIDs exercised and verified end-to-end by this harness were changed. The
  sibling interface IIDs used off this path — `IID_IEDIT_CONTROLLER`,
  `IID_IBSTREAM`, `IID_ICOMPONENT_HANDLER`, `IID_IPLUG_VIEW`, `IID_IPLUG_FRAME`,
  `IID_IHOST_APPLICATION` — carry the **same** defect and will likewise need
  `interface_iid()` (plus their own validation) before VST3 state save/load (ADR-010)
  and the VST3 editor GUI work on Windows. Tracked under open audit item **V4**.

> **Update 2026-10-21.** All interface IIDs now go through `interface_iid()`.
> (`IID_IPLUG_VIEW` was never defined; the view comes from `createView`.) The
> swap alone was not enough: the literals of `IEditController`,
> `IConnectionPoint`, `IHostApplication`, `IComponentHandler` and `IPlugFrame`
> were themselves wrong. They were corrected and verified byte for byte in the
> TDR Nova binary.
>
> - **Guard.** `interface_iids_match_the_sdk` (`vst3/ffi.rs`) compares all nine
>   IIDs to their SDK `DECLARE_CLASS_IID` form.
> - **Real-plugin check.** `vst3_real_instance_full_lifecycle` now also checks
>   the edit controller (T0): TDR Nova lists 76 parameters with their text, and a
>   state round-trip keeps their values.
> - **Stale probe.** `vst3_iid_byteorder_probe` swaps an IID that is already in
>   COM order, so its "engine vs swapped" labels are inverted. It only runs when
>   every candidate fails to instantiate.

## Divergences: bypass/mock path vs. real FFI

| Aspect                  | Existing suite (internal/pass-through) | Real FFI (this harness)                                       |
| ----------------------- | -------------------------------------- | ------------------------------------------------------------- |
| Boundary crossed        | none (`PassThroughDevice`)             | real `LoadLibrary` + factory + `process` vtable               |
| Activation              | not exercised                          | CLAP + VST3: real activate/start_processing ✅                |
| SR / block re-init      | not exercised                          | CLAP + VST3: real deactivate→reactivate ✅                    |
| Teardown                | `Drop` is a no-op for `Internal`       | CLAP + VST3: real release sequence ✅                         |
| VST3 instantiation      | never tested                           | was broken (F1) — found, root-caused, **fixed**, now verified |
| Bridge passthrough test | synthetic CLAP vtable (no DLL)         | real `.clap` loaded from disk                                 |

## Risks detected

1. **R1 (was critical) — VST3 hosting broken on Windows (F1). RESOLVED** for the
   instantiation/activation/process/teardown path. Residual: the off-path
   interface IIDs (state save/load, editor GUI, host context) still need the same
   `interface_iid()` treatment + validation (audit V4) before those features work
   on Windows.
2. **R2 — teardown "no leak" is structural, not absolute.** T5 proves the last
   `Arc` is the only remaining reference and that `PluginInstance::Drop` runs the
   real release sequence without crash/hang. It does **not** use an OS handle /
   heap leak detector — a plugin that leaks _internally_ on `destroy` would not be
   caught here. Adding a handle-count assertion is future work.
3. **R3 — instrument-vs-effect coverage.** CLAP was validated with an _effect_
   (clean audio-in→out). The instrument path (`ClapInstrument`, MIDI-driven) is
   activated identically but its note/voice lifecycle is not covered by this
   harness.
4. **R4 — single-plugin breadth.** Only one CLAP and one VST3 were available on
   the test machine. The byte-order finding is format-systemic (proven by the
   IID probe, independent of the specific plugin), but broader plugin coverage
   (dual-component VST3, plugins requiring `IHostApplication`) is recommended once
   F1 is fixed.

## Status

- CLAP real lifecycle (T1–T5): **PASS**.
- VST3 real lifecycle (T1–T5): **PASS** (after the F1 fix).
- Full suite: **70 passed, 0 failed, 0 ignored**.
- Engine change: minimal byte-order fix to two VST3 interface IIDs
  (`cfg(windows)`-gated, [`vst3/ffi.rs`](../../../src-tauri/src/plugin_formats/vst3/ffi.rs)).
  No graph, scheduler, threading, or plugin-system design was changed.
