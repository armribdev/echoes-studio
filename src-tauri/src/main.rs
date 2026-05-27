mod audio_clip;
mod project;
mod windows_theme;

use audio_clip::AudioClip;
use project::{
    AppClip, AppProject, AppState, AppTrack, ClipDto, ProjectDto, ProjectFile, TrackDto,
    generate_id,
};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager, Theme};
use tauri_plugin_dialog::{DialogExt, FilePath};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .setup(|app| {
            // ── Debug auto-load ───────────────────────────────────────────────
            // Configured via .cargo/config.toml → ECHOES_DEBUG_PROJECT env var.
            // State is populated before the window loads; the frontend's initial
            // invoke('get_project') picks it up without needing an extra event.
            let mut initial_title = "Sans titre".to_string();

            #[cfg(debug_assertions)]
            if let Ok(path_str) = std::env::var("ECHOES_DEBUG_PROJECT") {
                let path = std::path::PathBuf::from(&path_str);
                if path.exists() {
                    match load_project_from_file(&path) {
                        Ok(project) => {
                            initial_title = project.name.clone();
                            if let Ok(mut p) = app.state::<AppState>().project.lock() {
                                *p = project;
                            }
                            if let Ok(mut p) = app.state::<AppState>().project_path.lock() {
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

            // Popups « Fichier » etc. : ForceDark via uxtheme (AllowDark ne suffit pas si Windows est clair).
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
            get_audio_clip_duration,
            get_project,
            update_project_name,
            update_tempo,
            add_track,
            remove_track,
            update_track,
            import_clip,
            remove_clip,
            move_clip,
            save_project,
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

// ── Project file loading (shared by menu handler and debug auto-load) ─────────

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
    if let Ok(mut p) = state.project.lock() { *p = project; }
    if let Ok(mut p) = state.project_path.lock() { *p = Some(file_path); }
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
            if let Ok(mut p) = state.project.lock() { *p = new_proj; }
            if let Ok(mut p) = state.project_path.lock() { *p = None; }
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
                std::fs::write(&save_path, json.as_bytes()).map_err(|e| {
                    serde_json::Error::io(e)
                })
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

        _ => {}
    }
}

// ── Utility commands ─────────────────────────────────────────────────────────

#[tauri::command]
fn get_audio_clip_duration(file_path: &str) -> Result<f64, String> {
    let file = std::fs::File::open(file_path).map_err(|e| e.to_string())?;
    let reader = hound::WavReader::new(file).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    Ok(reader.duration() as f64 / spec.sample_rate as f64)
}

// ── Project state commands ────────────────────────────────────────────────────

#[tauri::command]
fn get_project(state: tauri::State<'_, AppState>) -> Result<ProjectDto, String> {
    let project = state.project.lock().map_err(|e| e.to_string())?;
    Ok(project.to_dto())
}

#[tauri::command]
fn update_project_name(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<(), String> {
    {
        let mut project = state.project.lock().map_err(|e| e.to_string())?;
        project.name = name.clone();
    }
    set_window_title(&app, &name);
    Ok(())
}

#[tauri::command]
fn update_tempo(
    state: tauri::State<'_, AppState>,
    bpm: f32,
    numerator: u8,
    denominator: u8,
) -> Result<(), String> {
    let mut project = state.project.lock().map_err(|e| e.to_string())?;
    project.bpm = bpm;
    project.numerator = numerator;
    project.denominator = denominator;
    Ok(())
}

#[tauri::command]
fn add_track(state: tauri::State<'_, AppState>, name: String) -> Result<TrackDto, String> {
    let mut project = state.project.lock().map_err(|e| e.to_string())?;
    let track = AppTrack { name, volume: 80.0, clips: vec![] };
    let dto = track.to_dto();
    project.tracks.push(track);
    Ok(dto)
}

#[tauri::command]
fn remove_track(state: tauri::State<'_, AppState>, track_index: usize) -> Result<(), String> {
    let mut project = state.project.lock().map_err(|e| e.to_string())?;
    if track_index >= project.tracks.len() {
        return Err("Index de piste invalide".to_string());
    }
    project.tracks.remove(track_index);
    Ok(())
}

#[tauri::command]
fn update_track(
    state: tauri::State<'_, AppState>,
    track_index: usize,
    name: String,
    volume: f32,
) -> Result<(), String> {
    let mut project = state.project.lock().map_err(|e| e.to_string())?;
    let track = project.tracks.get_mut(track_index).ok_or("Index de piste invalide")?;
    track.name = name;
    track.volume = volume;
    Ok(())
}

// ── Clip commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn import_clip(
    state: tauri::State<'_, AppState>,
    track_index: usize,
    file_path: String,
    start: f64,
    end: f64,
) -> Result<ClipDto, String> {
    let audio = AudioClip::from_wav_file(&file_path).map_err(|e| e.to_string())?;
    let data = audio.into_clip_data();

    let clip = AppClip {
        id: generate_id(),
        start,
        end,
        file_path,
        raw_samples: data.raw_samples,
        sample_rate: data.sample_rate,
        peaks_64: data.peaks_64,
        peaks_512: data.peaks_512,
        peaks_4096: data.peaks_4096,
    };

    let dto = clip.to_dto();

    let mut project = state.project.lock().map_err(|e| e.to_string())?;
    let track = project.tracks.get_mut(track_index).ok_or("Index de piste invalide")?;
    track.clips.push(clip);

    Ok(dto)
}

#[tauri::command]
fn remove_clip(
    state: tauri::State<'_, AppState>,
    track_index: usize,
    clip_id: String,
) -> Result<(), String> {
    let mut project = state.project.lock().map_err(|e| e.to_string())?;
    let track = project.tracks.get_mut(track_index).ok_or("Index de piste invalide")?;
    track.clips.retain(|c| c.id != clip_id);
    Ok(())
}

#[tauri::command]
fn move_clip(
    state: tauri::State<'_, AppState>,
    track_index: usize,
    clip_id: String,
    start: f64,
    end: f64,
) -> Result<(), String> {
    let mut project = state.project.lock().map_err(|e| e.to_string())?;
    let track = project.tracks.get_mut(track_index).ok_or("Index de piste invalide")?;
    let clip = track.clips.iter_mut().find(|c| c.id == clip_id).ok_or("Clip introuvable")?;
    clip.start = start;
    clip.end = end;
    Ok(())
}

// ── Persistence command (programmatic save with optional path) ───────────────

#[tauri::command]
fn save_project(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: Option<String>,
) -> Result<String, String> {
    let save_path = if let Some(p) = path {
        std::path::PathBuf::from(p)
    } else {
        let picked = app
            .dialog()
            .file()
            .add_filter("Echoes Studio Project", &["echoes"])
            .blocking_save_file();
        match picked {
            Some(FilePath::Path(p)) => p,
            _ => return Err("Annulé".to_string()),
        }
    };

    let save_path = if save_path.extension().map_or(true, |e| e != "echoes") {
        save_path.with_extension("echoes")
    } else {
        save_path
    };

    let project_file = {
        let project = state.project.lock().map_err(|e| e.to_string())?;
        project.to_file()
    };

    let json = serde_json::to_string_pretty(&project_file).map_err(|e| e.to_string())?;
    std::fs::write(&save_path, json.as_bytes()).map_err(|e| e.to_string())?;

    *state.project_path.lock().map_err(|e| e.to_string())? = Some(save_path.clone());

    Ok(save_path.to_string_lossy().to_string())
}
