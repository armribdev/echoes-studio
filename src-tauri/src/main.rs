mod audio_clip;
mod audio_engine;
mod engine_gateway;
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
            // ── Debug auto-load ───────────────────────────────────────────────
            let mut initial_title = "Sans titre".to_string();

            #[cfg(debug_assertions)]
            if let Ok(path_str) = std::env::var("ECHOES_DEBUG_PROJECT") {
                let path = std::path::PathBuf::from(&path_str);
                if path.exists() {
                    match load_project_from_file(&path) {
                        Ok(project) => {
                            initial_title = project.name.clone();
                            {
                                let state = app.state::<AppState>();
                                let mut p = state.project.lock().unwrap();
                                *p = project;
                            }
                            {
                                let state = app.state::<AppState>();
                                let mut p = state.project_path.lock().unwrap();
                                *p = Some(path);
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
                    Err(e) => eprintln!("Erreur ouverture projet: {}", e),
                }
            }
        }

        "undo" => {
            let result = undo(app.clone(), state.clone());
            match result {
                Ok(_snapshot) => {} // UI update via engine-event PROJECT_RESET
                Err(e) => eprintln!("Undo error: {e}"),
            }
        }

        "redo" => {
            let result = redo(app.clone(), state.clone());
            match result {
                Ok(_snapshot) => {}
                Err(e) => eprintln!("Redo error: {e}"),
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
                eprintln!("Erreur sauvegarde: {}", e);
            }
        }

        _ => {
            eprintln!("[keymap] action inconnue : {menu_id:?}");
        }
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
