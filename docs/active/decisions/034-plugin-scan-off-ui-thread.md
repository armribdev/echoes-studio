# ADR-034 — The plugin scan runs off the UI thread and outside the engine mutex

## Status

**Accepted — 2026-11-06** (plan approved by the user the same day; the four open points were left to the
choice of a modern pro DAW: background scan at start, plugins listed as the scan goes, a rescan that lists only
new or changed files, scan processes tied to the application). Implemented the same day; not yet seen on screen.
Amends ADR-033, step 10.

## Context

ADR-033 step 10 moved the loading of each plugin file into a process of its own (`plugin_host_bridge --scan`,
15 s limit). The processes were started from places that still blocked everything else:

- `scan_plugins` and `reenable_blocked_plugin` were synchronous Tauri commands. A synchronous command runs on
  the thread that serves the window (Tauri 2.11.2 separates `sync` from `sync_threadpool`, which is what
  `#[tauri::command(async)]` gives), so the window froze for the whole scan.
- They, and the scan at start, ran the loop under `audio_engine.lock()`. Every command that needs the engine
  (26 places in `main.rs`, 3 in `engine_gateway/api.rs`, among them every recompile) waited behind the slowest plugin.
  The audio thread never takes that mutex, so a project already playing kept playing, but nothing could be
  changed. `threading-model.md` already forbade it ("Cannot do: hold `audio_engine` lock while doing expensive
  I/O"); ADR-033 step 10 had written the opposite ("it holds the engine mutex while it runs, as the old
  in-constructor scan did").
- The start-up scan ran in Tauri's `setup`, before the window could answer.
- A scan process that hung had no owner once the application died: the 15 s kill belonged to a thread of the
  application.
- A rescan forgot the cache, so every file got a process again even though a new or changed file is never in the
  cache (it is keyed on path, size and modification time).

## Decision

1. **`PluginScanner` owns the scan** (`plugin_system/scan.rs`), held by `AppState` next to `audio_engine`, not
   by `PluginHost`. It holds the bridge path, the time limit, the blocklist and cache (`ScanStore`) and the
   files to announce. `PluginHost` keeps only the registry and gains `register_scanned`.
2. **Two locks, none of them held while a scan process runs.** A *session* lock lets one scan (or one
   re-enable) run at a time; the others wait. A *state* lock guards the blocklist, cache and announcements for
   the length of a lookup or a write.
3. **Plugins are published file by file.** `scan(publish)` calls `publish(manifests)` as soon as a file is
   listed (or read from the cache); `AppState::scan_plugins` makes it take the engine mutex for one registry
   update. A project opened during the scan finds the earlier files already. Files are scanned in path order.
   The blocklist and cache are saved after every file, so a block survives an application that is closed or
   killed during the scan.
4. **Lock order.** session → (scanner state | engine), never the last two together; the scan takes neither the
   project nor the engine while a process runs. `recompile_audio_graph` (project → engine) runs after the scan,
   outside every scanner lock.
5. **Commands.** `scan_plugins` and `reenable_blocked_plugin` are `#[tauri::command(async)]`.
   `list_blocked_plugins` and `take_newly_blocked_plugins` read the scanner only; `plugin_scan_running` is new.
6. **The scan starts in the background.** `setup` points the scanner at its folder and spawns a thread that
   scans, recompiles (so the plugins of an open project load) and emits `plugin-scan-finished`. The window
   opens at once. The interface follows with `followStartupPluginScan`: it stays "loading"
   (`pluginsLoading`, now a count of overlapping jobs) until the event, then reads the list and announces
   blocked files. `MissingPluginsDialog` does not judge a plugin missing while `pluginsLoading` is true.
7. **A rescan lists new or changed files only.** `rescan_plugins` and `ScanStore::forget_scanned` are gone;
   blocked files stay blocked until changed or re-enabled, as before.
8. **The scan processes belong to a Windows job object** (`KILL_ON_JOB_CLOSE`, `ScanJob`; `windows-sys` feature
   `Win32_System_JobObjects`). They end with the application, including a kill from the debugger. A process is
   attached just after it starts, so a helper it started in that instant is not covered.

## What does not change

No audio thread, no scheduler, snapshot or worker pool; no new lock on the audio path (C-1); the bridge
protocol and the formats of `plugin-blocklist.json` and `plugin-scan-cache.json` are untouched; the golden
hashes `vst3_plugins` and `clap_plugins` do not move. The scan stays on a command-side thread: no hard
invariant of `architecture_rules.md` changes (thread ownership is the same, the UI thread stops being one of
its users).

## Consequences

- The window never waits for a scan and the engine is never held by one. Proof:
  `scan_sandbox_tests::the_engine_stays_free_while_a_plugin_hangs_during_the_scan` (the engine mutex is free,
  the earlier plugin is registered, the blocklist is readable, while `hang_in_factory` is being scanned; it
  fails when the scan holds the engine mutex).
- At the very first start (no cache) the plugin lists fill after the window opens. A project opened in those
  seconds shows its plugins as unavailable (state kept, ADR-010) and gets them back at the recompile that ends
  the scan; the missing-plugins dialog waits for it.
- Two scans never overlap: a second one waits, then finds the work done (cache and blocklist).
- A killed application can no longer leave a hung scan process behind.

## Not done

Seen on screen: first start with the installed plugin set (TDR Nova, Surge XT, Vital, the two test plugins in
the VST3 folder), the time of a cold scan, closing the application during a scan. No progress display, no way to
cancel a scan; neither was asked for.
