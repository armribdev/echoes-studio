mod audio_clip;
mod audio_engine;
mod audio_graph;
mod audio_settings;
mod engine_gateway;
mod plugin_formats;
mod plugin_system;
mod project;
mod windows_theme;

use engine_gateway::api::{get_snapshot, redo, send_batch, send_command, undo};
use project::{AppProject, AppState, ProjectDto, ProjectFile};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager, Theme};
use tauri_plugin_dialog::{DialogExt, FilePath};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .setup(|app| {
            // ── Native audio engine ───────────────────────────────────────────
            // Start the CPAL output stream as early as possible so the engine
            // is ready before any project is loaded.  Failure is logged but
            // non-fatal: the app continues and export (via OfflineAudioContext) still works.
            start_native_audio(app);

            // ── Debug auto-load ───────────────────────────────────────────────
            let mut initial_title = "Sans titre".to_string();

            #[cfg(debug_assertions)]
            if let Ok(path_str) = std::env::var("ECHOES_DEBUG_PROJECT") {
                let path = std::path::PathBuf::from(&path_str);
                if path.exists() {
                    match load_project_from_file(&path) {
                        Ok(project) => {
                            initial_title = project.name.clone();
                            // Seed the event store so undo can return to the
                            // loaded state instead of wiping back to empty.
                            {
                                let state = app.state::<AppState>();
                                if let Ok(mut store) = state.event_store.lock() {
                                    store.init_from_project(
                                        engine_gateway::api::snapshot_project_for_store(&project),
                                    );
                                };
                            }
                            {
                                let state = app.state::<AppState>();
                                if let Ok(mut p) = state.project.lock() {
                                    *p = project;
                                };
                            }
                            {
                                let state = app.state::<AppState>();
                                if let Ok(mut p) = state.project_path.lock() {
                                    *p = Some(path);
                                };
                            }
                            // Recompile the audio graph so CPAL knows about the
                            // clips in the loaded project (fixes silent playback
                            // after ECHOES_DEBUG_PROJECT auto-load).
                            {
                                let state = app.state::<AppState>();
                                recompile_audio_graph(&state);
                            }
                        }
                        Err(e) => eprintln!("[debug] Échec chargement {path_str}: {e}"),
                    }
                } else {
                    eprintln!("[debug] ECHOES_DEBUG_PROJECT introuvable: {path_str}");
                }
            }

            set_window_title(app.handle(), &initial_title);

            windows_theme::force_dark_popups();

            let file_menu = Submenu::with_items(
                app,
                "Fichier",
                true,
                &[
                    &MenuItem::with_id(app, "new", "Nouveau projet", true, Some("CmdOrCtrl+N"))?,
                    &MenuItem::with_id(app, "open", "Ouvrir...", true, Some("CmdOrCtrl+O"))?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "import-audio", "Importer audio...", true, None::<&str>)?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "save", "Enregistrer", true, Some("CmdOrCtrl+S"))?,
                    &MenuItem::with_id(app, "save-as", "Enregistrer sous...", true, None::<&str>)?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "undo", "Annuler", true, Some("CmdOrCtrl+Z"))?,
                    &MenuItem::with_id(app, "redo", "Rétablir", true, Some("CmdOrCtrl+Shift+Z"))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?;

            let menu = Menu::with_items(app, &[&file_menu])?;
            app.set_menu(menu)?;

            apply_native_dark_theme(app.handle());

            Ok(())
        })
        .on_menu_event(|app, event| {
            let app = app.clone();
            let menu_id = event.id.0.clone();
            std::thread::spawn(move || {
                handle_menu_event(&app, &menu_id);
            });
        })
        .invoke_handler(tauri::generate_handler![
            // ── Read-only helpers ─────────────────────────────────────────────
            get_audio_clip_duration,
            get_project,
            // ── Gateway commands (all mutations go here) ──────────────────────
            send_command,
            send_batch,
            get_snapshot,
            undo,
            redo,
            // ── Keyboard shortcut bridge ──────────────────────────────────────
            menu_action,
            // ── Export ────────────────────────────────────────────────────────
            export_wav,
            // ── Audio settings ────────────────────────────────────────────────
            get_audio_devices,
            get_device_sample_rates,
            get_device_channels,
            apply_audio_settings,
            set_active_output_channels,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ── Window title ─────────────────────────────────────────────────────────────

fn apply_native_dark_theme(app: &tauri::AppHandle) {
    app.set_theme(Some(Theme::Dark));
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_theme(Some(Theme::Dark));
    }
    #[cfg(windows)]
    windows_theme::force_dark_popups();
}

fn set_window_title(app: &tauri::AppHandle, project_name: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let title = format!("{} — Echoes Studio", project_name);
        let _ = window.set_title(&title);
    }
}

// ── Project file loading ──────────────────────────────────────────────────────

fn load_project_from_file(path: &std::path::Path) -> Result<AppProject, String> {
    let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let pf: ProjectFile = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    AppProject::from_file(pf)
}

fn apply_loaded_project(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    project: AppProject,
    file_path: std::path::PathBuf,
) {
    let name = project.name.clone();
    let dto = project.to_dto();

    // Seed event store from loaded project (snapshot-at-zero approach)
    if let (Ok(mut store), Ok(mut p)) =
        (state.event_store.lock(), state.project.lock())
    {
        store.init_from_project(engine_gateway::api::snapshot_project_for_store(&project));
        *p = project;
    }

    if let Ok(mut p) = state.project_path.lock() {
        *p = Some(file_path);
    }

    // Recompile the audio graph so the engine knows about the loaded clips.
    // Without this the scheduler stays at the empty-project state and produces
    // silence even though the UI shows the correct tracks and clips.
    recompile_audio_graph(state);

    set_window_title(app, &name);
    let _ = app.emit("project-loaded", dto);
}

// ── Native menu event handler ─────────────────────────────────────────────────

fn handle_menu_event(app: &tauri::AppHandle, menu_id: &str) {
    let state = app.state::<AppState>();

    match menu_id {
        "new" => {
            let new_proj = AppProject::new();
            let dto = new_proj.to_dto();
            if let Ok(mut store) = state.event_store.lock() {
                store.init_from_project(AppProject::new());
            }
            if let Ok(mut p) = state.project.lock() {
                *p = new_proj;
            }
            if let Ok(mut p) = state.project_path.lock() {
                *p = None;
            }
            recompile_audio_graph(&state);
            set_window_title(app, "Sans titre");
            let _ = app.emit("project-loaded", dto);
        }

        "open" => {
            let picked = app
                .dialog()
                .file()
                .add_filter("Echoes Studio Project", &["echoes"])
                .blocking_pick_file();

            if let Some(FilePath::Path(file_path)) = picked {
                match load_project_from_file(&file_path) {
                    Ok(project) => apply_loaded_project(app, &state, project, file_path),
                    Err(e) => eprintln!("[projet] Échec ouverture : {e}"),
                }
            }
        }

        "undo" => {
            let result = undo(app.clone(), state.clone());
            match result {
                Ok(_snapshot) => {}
                Err(e) => eprintln!("[projet] Undo échoué : {e}"),
            }
        }

        "redo" => {
            let result = redo(app.clone(), state.clone());
            match result {
                Ok(_snapshot) => {}
                Err(e) => eprintln!("[projet] Redo échoué : {e}"),
            }
        }

        "save" | "save-as" => {
            let force_dialog = menu_id == "save-as";
            let current_path = if force_dialog {
                None
            } else {
                state.project_path.lock().ok().and_then(|p| p.clone())
            };

            let save_path = if let Some(p) = current_path {
                p
            } else {
                match app
                    .dialog()
                    .file()
                    .add_filter("Echoes Studio Project", &["echoes"])
                    .blocking_save_file()
                {
                    Some(FilePath::Path(p)) => p,
                    _ => return,
                }
            };

            let save_path = if save_path.extension().map_or(true, |e| e != "echoes") {
                save_path.with_extension("echoes")
            } else {
                save_path
            };

            let write_result = {
                let project = state.project.lock().unwrap();
                serde_json::to_string_pretty(&project.to_file())
            }
            .and_then(|json| {
                std::fs::write(&save_path, json.as_bytes()).map_err(serde_json::Error::io)
            });

            if write_result.is_ok() {
                if let Ok(mut p) = state.project_path.lock() {
                    *p = Some(save_path.clone());
                }
                let _ = app.emit("project-saved", save_path.to_string_lossy().to_string());
            } else if let Err(e) = write_result {
                eprintln!("[projet] Échec sauvegarde : {e}");
            }
        }

        "import-audio" => {
            let picked = app
                .dialog()
                .file()
                .add_filter("Fichiers audio", &["wav"])
                .blocking_pick_file();

            if let Some(FilePath::Path(file_path)) = picked {
                let path_str = file_path.to_string_lossy().to_string();
                let _ = app.emit("audio-file-picked", path_str);
            }
        }

        _ => {} // actions inconnues ignorées silencieusement
    }
}

// ── Tauri commands ────────────────────────────────────────────────────────────

/// Keyboard shortcut bridge — mirrors every native menu accelerator so the
/// frontend keymap module can invoke the exact same logic.
///
/// File dialogs block the calling thread, so we spawn a dedicated thread the
/// same way `on_menu_event` does.  The command returns immediately; results
/// are communicated back to the frontend via the existing Tauri event channel
/// ("project-loaded", "project-saved", …).
#[tauri::command]
fn menu_action(app: tauri::AppHandle, action: String) {
    std::thread::spawn(move || {
        handle_menu_event(&app, &action);
    });
}

/// Duration of a WAV file in seconds — used for drag-and-drop preview.
#[tauri::command]
fn get_audio_clip_duration(file_path: &str) -> Result<f64, String> {
    let file = std::fs::File::open(file_path).map_err(|e| e.to_string())?;
    let reader = hound::WavReader::new(file).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    Ok(reader.duration() as f64 / spec.sample_rate as f64)
}

/// Full project snapshot for initial frontend hydration.
/// All subsequent mutations go through `send_command`.
#[tauri::command]
fn get_project(state: tauri::State<'_, AppState>) -> Result<ProjectDto, String> {
    let project = state.project.lock().map_err(|e| e.to_string())?;
    Ok(project.to_dto())
}

/// Write a stereo mixdown as a WAV file at the requested bit depth.
///
/// | `bit_depth` | WAV format            | Notes                          |
/// |-------------|-----------------------|--------------------------------|
/// | 16          | 16-bit PCM (int)      | CD quality, widest compat      |
/// | 24          | 24-bit PCM (int)      | Pro standard for delivery      |
/// | 32          | 32-bit float          | Archival / stem exports        |
///
/// Called by the frontend after rendering the project offline via
/// `OfflineAudioContext`. The path is chosen by the user via the JS save
/// dialog before this command is invoked.
#[tauri::command]
fn export_wav(
    path: String,
    samples_left: Vec<f32>,
    samples_right: Vec<f32>,
    sample_rate: u32,
    bit_depth: u8,
) -> Result<(), String> {
    let path = std::path::PathBuf::from(&path);
    let path = if path.extension().map_or(true, |e| e != "wav") {
        path.with_extension("wav")
    } else {
        path
    };

    match bit_depth {
        32 => {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut w = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
            for (l, r) in samples_left.iter().zip(samples_right.iter()) {
                w.write_sample(*l).map_err(|e| e.to_string())?;
                w.write_sample(*r).map_err(|e| e.to_string())?;
            }
            w.finalize().map_err(|e| e.to_string())
        }
        24 => {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate,
                bits_per_sample: 24,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
            for (l, r) in samples_left.iter().zip(samples_right.iter()) {
                // Scale to 24-bit signed integer range [-8 388 608, +8 388 607]
                let l_i = (l.clamp(-1.0, 1.0) * 8_388_607.0) as i32;
                let r_i = (r.clamp(-1.0, 1.0) * 8_388_607.0) as i32;
                w.write_sample(l_i).map_err(|e| e.to_string())?;
                w.write_sample(r_i).map_err(|e| e.to_string())?;
            }
            w.finalize().map_err(|e| e.to_string())
        }
        _ => {
            // 16-bit or anything unrecognised
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
            for (l, r) in samples_left.iter().zip(samples_right.iter()) {
                let l_i = (l.clamp(-1.0, 1.0) * 32_767.0) as i16;
                let r_i = (r.clamp(-1.0, 1.0) * 32_767.0) as i16;
                w.write_sample(l_i).map_err(|e| e.to_string())?;
                w.write_sample(r_i).map_err(|e| e.to_string())?;
            }
            w.finalize().map_err(|e| e.to_string())
        }
    }
}

// ── Native audio bootstrap ────────────────────────────────────────────────────

/// Start the CPAL audio engine during Tauri `setup`.
///
/// Uses a dedicated function so the compiler can clearly track the lifetime of
/// the temporary `tauri::State` (avoids the "does not live long enough" error
/// that occurs when the match arm's `MutexGuard<PoisonError<…>>` outlives the
/// `State` in an anonymous block).
fn start_native_audio(app: &mut tauri::App) {
    let app_handle = app.handle().clone();
    let state = app.state::<AppState>();

    // Read stored settings (default on first launch, persisted after that).
    let settings = state
        .audio_settings
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();

    // Build a minimal initial project — the real graph is pushed by
    // `recompile_audio_graph` once a project is loaded or created.
    let init_project = AppProject::new();

    if let Ok(mut engine) = state.audio_engine.lock() {
        if let Err(e) = engine.start_stream_with_settings(&init_project, app_handle, &settings) {
            eprintln!("[audio] Impossible d'activer le moteur natif : {e}");
        }
    };
    // Note: the semicolon above is intentional.  It forces the MutexGuard
    // temporary (inside the Result returned by .lock()) to drop before `state`
    // is dropped at function exit, satisfying the borrow checker.
}

// ── Audio settings commands ───────────────────────────────────────────────────

/// Return the list of available input and output audio devices.
///
/// The frontend uses this to populate the device pickers in the Settings panel.
/// Device IDs are the CPAL device names; `"default"` is prepended as a
/// special sentinel that always resolves to the system default.
#[tauri::command]
fn get_audio_devices() -> Result<audio_settings::AudioDeviceList, String> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();

    let mut outputs = vec![audio_settings::AudioDeviceInfo {
        id: "default".to_string(),
        name: "Par défaut (système)".to_string(),
    }];
    if let Ok(devices) = host.output_devices() {
        for d in devices {
            if let Ok(name) = d.name() {
                outputs.push(audio_settings::AudioDeviceInfo {
                    id: name.clone(),
                    name,
                });
            }
        }
    }

    let mut inputs = vec![audio_settings::AudioDeviceInfo {
        id: "default".to_string(),
        name: "Par défaut (système)".to_string(),
    }];
    if let Ok(devices) = host.input_devices() {
        for d in devices {
            if let Ok(name) = d.name() {
                inputs.push(audio_settings::AudioDeviceInfo {
                    id: name.clone(),
                    name,
                });
            }
        }
    }

    Ok(audio_settings::AudioDeviceList { inputs, outputs })
}

/// Return which of the four candidate sample rates are supported by a device.
///
/// Candidate list: 44 100, 48 000, 88 200, 96 000 Hz — the same values shown
/// in the Settings panel.  The frontend uses the result to grey out unsupported
/// pills so the user can only pick rates the device actually handles.
///
/// If the device cannot be queried (permissions, driver issue, etc.) all four
/// candidates are returned so the UI stays fully enabled rather than crashing.
#[tauri::command]
fn get_device_sample_rates(device_id: String) -> Vec<u32> {
    use cpal::traits::{DeviceTrait, HostTrait};

    const CANDIDATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

    let host = cpal::default_host();

    let device = if device_id == "default" {
        host.default_output_device()
    } else {
        host.output_devices()
            .ok()
            .and_then(|mut it| {
                it.find(|d| d.name().map(|n| n == device_id).unwrap_or(false))
            })
            .or_else(|| host.default_output_device())
    };

    let Some(device) = device else {
        return CANDIDATES.to_vec();
    };

    let Ok(ranges) = device.supported_output_configs() else {
        return CANDIDATES.to_vec();
    };

    // Collect ranges once — `supported_output_configs()` is an iterator.
    let ranges: Vec<_> = ranges.collect();
    if ranges.is_empty() {
        return CANDIDATES.to_vec();
    }

    // Only count a sample rate as "supported" when it is available in F32 or
    // I16 — the two formats the engine actually handles without quality loss.
    // An exotic format (U8, U16 …) at a given SR would not produce usable audio,
    // so we don't advertise it as a valid choice in the UI.
    let supported: Vec<u32> = CANDIDATES
        .iter()
        .filter(|&&sr| {
            let csr = cpal::SampleRate(sr);
            ranges.iter().any(|r| {
                matches!(
                    r.sample_format(),
                    cpal::SampleFormat::F32 | cpal::SampleFormat::I16
                ) && r.min_sample_rate() <= csr
                    && r.max_sample_rate() >= csr
            })
        })
        .copied()
        .collect();

    // If nothing matched (very unusual device), return all candidates so the
    // UI never shows a fully-greyed-out pill group.
    if supported.is_empty() {
        CANDIDATES.to_vec()
    } else {
        supported
    }
}

/// Return the native channel count of a device.
///
/// For output devices the default output config is queried; for input devices
/// the default input config.  Falls back to 2 (stereo) on any error so the UI
/// always shows a sensible value.
#[tauri::command]
fn get_device_channels(device_id: String, is_input: bool) -> u16 {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();

    if is_input {
        let device = if device_id == "default" {
            host.default_input_device()
        } else {
            host.input_devices()
                .ok()
                .and_then(|mut it| {
                    it.find(|d| d.name().map(|n| n == device_id).unwrap_or(false))
                })
                .or_else(|| host.default_input_device())
        };
        device
            .and_then(|d| d.default_input_config().ok())
            .map(|c| c.channels())
            .unwrap_or(2)
    } else {
        let device = if device_id == "default" {
            host.default_output_device()
        } else {
            host.output_devices()
                .ok()
                .and_then(|mut it| {
                    it.find(|d| d.name().map(|n| n == device_id).unwrap_or(false))
                })
                .or_else(|| host.default_output_device())
        };
        device
            .and_then(|d| d.default_output_config().ok())
            .map(|c| c.channels())
            .unwrap_or(2)
    }
}

/// Apply new audio settings: persist them and, only if necessary, restart the
/// CPAL stream.
///
/// A stream restart (brief ~50 ms audio interruption) is triggered **only**
/// when a parameter that cannot be changed atomically differs from the current
/// value:
///   - output or input device
///   - sample rate
///   - buffer size
///
/// `output_channels` is handled by an atomic inside the callback and was
/// already applied by `set_active_output_channels` during the live-preview
/// phase — so saving never causes a glitch when only the channel count changed.
///
/// Returns an error string if the device is unavailable or the stream cannot
/// be opened; the UI should surface this as a notification.
#[tauri::command]
fn apply_audio_settings(
    settings: audio_settings::AudioSettings,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // Read current settings before overwriting so we can diff them.
    let current = state
        .audio_settings
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();

    // Persist for future app launches / restarts.
    if let Ok(mut s) = state.audio_settings.lock() {
        *s = settings.clone();
    }

    // Determine whether a stream restart is actually needed.
    let needs_restart = current.output_device != settings.output_device
        || current.input_device  != settings.input_device
        || current.sample_rate   != settings.sample_rate
        || current.buffer_size   != settings.buffer_size;

    if needs_restart {
        // Full restart: device / sample rate / buffer changed.
        // `start_stream_with_settings` also seeds the active-channels atomic,
        // so output_channels is always consistent after a restart.
        let init_project = AppProject::new();
        if let Ok(mut engine) = state.audio_engine.lock() {
            engine.start_stream_with_settings(&init_project, app, &settings)?;
        }
        recompile_audio_graph(&state);
    } else {
        // No restart needed: only `output_channels` (or no change at all).
        // The live-preview already applied it atomically, but we set it again
        // to guarantee consistency even if the preview path was skipped.
        if let Ok(engine) = state.audio_engine.lock() {
            engine.set_active_output_channels(settings.output_channels as usize);
        }
    }

    Ok(())
}

/// Update the active output channel count without restarting the CPAL stream.
///
/// Changes take effect within one audio callback block (~5 ms at typical
/// buffer sizes) — no stream interruption, no audio glitch.
///
/// Used by the Settings panel to give a live preview of mono/stereo routing
/// as the user clicks +/−, and to revert instantly if they cancel.
///
/// `count = 0` restores the "use all hardware channels" default.
#[tauri::command]
fn set_active_output_channels(
    count: u16,
    state: tauri::State<'_, AppState>,
) {
    if let Ok(engine) = state.audio_engine.lock() {
        engine.set_active_output_channels(count as usize);
    }
}

// ── Audio graph recompilation helper ─────────────────────────────────────────

/// Recompile the audio graph from the current project state and push the new
/// scheduler to the CPAL audio thread.
///
/// Must be called **outside** any held Mutex locks to avoid deadlocks.
/// Mirrors the logic in `engine_gateway::api::recompile_if_needed` but is
/// accessible from `main.rs` (where that private function is not visible).
fn recompile_audio_graph(state: &tauri::State<'_, AppState>) {
    if let Ok(mut engine) = state.audio_engine.lock() {
        if let Ok(project) = state.project.lock() {
            engine.update_from_project(&project);
        }
    }
}
