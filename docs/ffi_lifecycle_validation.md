# FFI Plugin Lifecycle Validation (real instances)

**Date:** 2026-06-04 · **Scope:** real VST3 / CLAP instance lifecycle on Windows.
**Harness:** [`src-tauri/src/plugin_system/ffi_lifecycle_tests.rs`](../src-tauri/src/plugin_system/ffi_lifecycle_tests.rs)
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

| | Test | CLAP (Surge XT Effects) | VST3 (TDR Nova) |
|--|------|--------------------------|------------------|
| T1 | Activation `Created → activate → start_processing` | ✅ `active=true` | ⛔ blocked before activation (see finding) |
| T2 | Real audio block through `process` vtable | ✅ output finite | ⛔ |
| T3 | Sample-rate reinit (48k→44.1k) | ✅ reactivated | ⛔ |
| T4 | Block-size change (512→256) | ✅ re-init | ⛔ |
| T5 | Teardown / no leak | ✅ `strong_count=1`, `Destroyed`, real FFI release, no crash | ⛔ |

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

## Finding F1 — VST3 cannot be instantiated on Windows (interface IID byte order)

Every real VST3 fails at instantiation:

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
in [`plugin_formats/vst3/ffi.rs`](../src-tauri/src/plugin_formats/vst3/ffi.rs):

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
  the *same* class with the COM-swapped IID and gets `S_OK`, isolating byte order
  as the sole cause.

### Why this was never caught

- `enumerate_classes` / scan uses `getClassInfo`, which needs **no** interface
  IID, so VST3 plugins scan and register fine — only instantiation fails.
- The existing tests instantiate `echoes.gain` (internal `PassThroughDevice`),
  which never crosses the VST3 FFI boundary. No test had ever driven a real VST3
  through `createInstance` — exactly Lacune A.

### Impact

- **VST3 hosting is non-functional on Windows.** No third-party VST3 can be
  instantiated, activated, processed, or torn down. Lacune A / Lacune C for VST3
  are **not validatable** until F1 is fixed; the VST3 lifecycle test is
  `#[ignore]`d with this reason and will enforce T1–T5 automatically once it is.
- CLAP is unaffected (CLAP is a pure C ABI with no COM byte-order rule).

### Proposed minimal patch (NOT applied — out of scope: touches `plugin_formats`)

Per the validation constraints (test/harness layer only, no plugin-system
change), this is a proposal for the engine owner, not an applied change:

- Smallest fix: under `#[cfg(windows)]`, store every **interface** IID used in
  `queryInterface` / `createInstance` in COM byte order (or apply a `com_swap`
  helper at the constant definitions). The harness already contains a verified
  `com_swap` (reverse `[0..4]`, `[4..6]`, `[6..8]`; keep `[8..16]`).
- Affected constants (all queried by IID): `IID_ICOMPONENT`,
  `IID_IAUDIO_PROCESSOR`, `IID_IEDIT_CONTROLLER`, `IID_IBSTREAM`,
  `IID_ICOMPONENT_HANDLER`, `IID_IPLUG_VIEW`, `IID_IPLUG_FRAME`,
  `IID_IHOST_APPLICATION`. `IID_FUNKNOWN`/`IID_IPLUGIN_FACTORY` are not used in an
  IID comparison on the failing path but should be converted for consistency.
- **Class CIDs must NOT be swapped** — they already arrive in COM order.
- This is a behaviour fix on a documented boundary; it should be recorded against
  the open audit item **V4** (and likely warrants an ADR note since it changes the
  VST3 instantiation contract).

## Divergences: bypass/mock path vs. real FFI

| Aspect | Existing suite (internal/pass-through) | Real FFI (this harness) |
|--------|----------------------------------------|--------------------------|
| Boundary crossed | none (`PassThroughDevice`) | real `LoadLibrary` + factory + `process` vtable |
| Activation | not exercised | CLAP: real `init/activate/start_processing` ✅ · VST3: blocked by F1 |
| SR / block re-init | not exercised | CLAP: real deactivate→reactivate ✅ |
| Teardown | `Drop` is a no-op for `Internal` | CLAP: real `stop/deactivate/destroy` ✅ · VST3: blocked |
| VST3 instantiation | never tested | **fails (F1)** — would have stayed hidden |
| Bridge passthrough test | synthetic CLAP vtable (no DLL) | real `.clap` loaded from disk |

## Risks detected

1. **R1 (critical) — VST3 hosting broken on Windows (F1).** Ships today as a
   non-working format; users cannot load any VST3. Blocks all VST3 lifecycle
   validation.
2. **R2 — teardown "no leak" is structural, not absolute.** T5 proves the last
   `Arc` is the only remaining reference and that `PluginInstance::Drop` runs the
   real release sequence without crash/hang. It does **not** use an OS handle /
   heap leak detector — a plugin that leaks *internally* on `destroy` would not be
   caught here. Adding a handle-count assertion is future work.
3. **R3 — instrument-vs-effect coverage.** CLAP was validated with an *effect*
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
- VST3 real lifecycle (T1–T5): **BLOCKED by F1** (engine defect), reproduced and
  root-caused; test `#[ignore]`d pending the engine fix.
- No production code modified. Harness is additive and test-gated.
