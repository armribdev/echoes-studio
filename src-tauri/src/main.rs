mod audio_clip;
mod audio_engine;
mod audio_graph;
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

/// Write a stereo mixdown as a 32-bit float WAV file.
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
) -> Result<(), String> {
    let path = std::path::PathBuf::from(&path);
    let path = if path.extension().map_or(true, |e| e != "wav") {
        path.with_extension("wav")
    } else {
        path
    };

    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
    for (l, r) in samples_left.iter().zip(samples_right.iter()) {
        writer.write_sample(*l).map_err(|e| e.to_string())?;
        writer.write_sample(*r).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())
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

    // Build a minimal snapshot of the initial project (empty tracks) so the
    // graph compiler can determine the sample rate before any file is loaded.
    let project_snapshot = state
        .project
        .lock()
        .map(|p| AppProject {
            name: p.name.clone(),
            bpm: p.bpm,
            numerator: p.numerator,
            denominator: p.denominator,
            tracks: vec![], // no clips yet — recompile follows on first load
        })
        .unwrap_or_else(|_| AppProject::new());

    if let Ok(mut engine) = state.audio_engine.lock() {
        if let Err(e) = engine.start_stream(&project_snapshot, app_handle) {
            eprintln!("[audio] Impossible d'activer le moteur natif : {e}");
        }
    };
    // Note: the semicolon above is intentional.  It forces the MutexGuard
    // temporary (inside the Result returned by .lock()) to drop before `state`
    // is dropped at function exit, satisfying the borrow checker.
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
