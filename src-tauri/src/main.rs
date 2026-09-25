mod audio_clip;
mod audio_engine;
mod audio_graph;
mod audio_settings;
mod engine_gateway;
mod menu_state;
mod plugin_formats;
mod plugin_system;
mod project;
mod windows_theme;

use engine_gateway::api::{
    get_graph_snapshot, get_snapshot, preview_midi_note, redo, send_batch, send_command, set_track_mix,
    undo,
};
use project::{AppProject, AppState, ProjectDto, ProjectFile, ProjectUiLayout};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager, Theme};
use tauri_plugin_dialog::{DialogExt, FilePath};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .setup(|app| {
            // Before any plugin code runs, so a crash in it can be attributed.
            let suspects = open_crash_marker(app);
            app.manage(CrashSuspects(std::sync::Mutex::new(suspects)));

            // Start the CPAL output stream as early as possible so the engine
            // is ready before any project is loaded.  Failure is logged but
            // non-fatal: the app continues and offline export still works
            // (it drives the graph directly, without needing the live stream).
            start_native_audio(app);
            watch_plugin_edits(app.handle().clone());

            let mut initial_title = "Sans titre".to_string();

            #[cfg(debug_assertions)]
            if let Ok(path_str) = std::env::var("ECHOES_DEBUG_PROJECT") {
                let path = std::path::PathBuf::from(&path_str);
                if path.exists() {
                    match load_project_from_file(&path) {
                        Ok(project) => {
                            initial_title = project.name.clone();
                            let state = app.state::<AppState>();
                            install_loaded_project(app.handle(), &state, project, path);
                        }
                        Err(e) => eprintln!("[debug] Échec chargement {path_str}: {e}"),
                    }
                } else {
                    eprintln!("[debug] ECHOES_DEBUG_PROJECT introuvable: {path_str}");
                }
            }

            set_window_title(app.handle(), &initial_title);

            windows_theme::force_dark_popups();

            let save_item =
                MenuItem::with_id(app, "save", "Enregistrer", false, Some("CmdOrCtrl+S"))?;
            let undo_item = MenuItem::with_id(app, "undo", "Annuler", false, Some("CmdOrCtrl+Z"))?;
            let redo_item =
                MenuItem::with_id(app, "redo", "Rétablir", false, Some("CmdOrCtrl+Shift+Z"))?;
            let delete_track_item = MenuItem::with_id(
                app,
                "delete-track",
                "Supprimer la piste sélectionnée",
                false,
                None::<&str>,
            )?;

            let save_h = save_item.clone();
            let undo_h = undo_item.clone();
            let redo_h = redo_item.clone();
            let delete_track_h = delete_track_item.clone();

            app.manage(menu_state::MenuHandles {
                set_undo: Box::new(move |e| undo_h.set_enabled(e)),
                set_redo: Box::new(move |e| redo_h.set_enabled(e)),
                set_save: Box::new(move |e| save_h.set_enabled(e)),
                set_delete_track: Box::new(move |e| delete_track_h.set_enabled(e)),
            });

            let export_menu = Submenu::with_items(
                app,
                "Exporter",
                true,
                &[
                    &MenuItem::with_id(
                        app,
                        "export-wav",
                        "WAV",
                        true,
                        Some("CmdOrCtrl+Shift+E"),
                    )?,
                    &MenuItem::with_id(
                        app,
                        "export-mp3",
                        "MP3",
                        true,
                        Some("CmdOrCtrl+Shift+M"),
                    )?,
                ],
            )?;

            let file_menu = Submenu::with_items(
                app,
                "Fichier",
                true,
                &[
                    &MenuItem::with_id(app, "new", "Nouveau projet", true, Some("CmdOrCtrl+N"))?,
                    &MenuItem::with_id(app, "open", "Ouvrir...", true, Some("CmdOrCtrl+O"))?,
                    &MenuItem::with_id(
                        app,
                        "project-settings",
                        "Projet...",
                        true,
                        Some("CmdOrCtrl+Shift+,"),
                    )?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(
                        app,
                        "import-audio",
                        "Importer audio...",
                        true,
                        None::<&str>,
                    )?,
                    &PredefinedMenuItem::separator(app)?,
                    &export_menu,
                    &PredefinedMenuItem::separator(app)?,
                    &save_item,
                    &MenuItem::with_id(app, "save-as", "Enregistrer sous...", true, None::<&str>)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?;

            let edit_menu = Submenu::with_items(
                app,
                "Édition",
                true,
                &[&undo_item, &redo_item],
            )?;

            let app_menu = Submenu::with_items(
                app,
                "Echoes Studio",
                true,
                &[&MenuItem::with_id(
                    app,
                    "preferences",
                    "Préférences...",
                    true,
                    Some("CmdOrCtrl+,"),
                )?],
            )?;

            let track_menu = Submenu::with_items(
                app,
                "Piste",
                true,
                &[
                    &MenuItem::with_id(
                        app,
                        "add-track",
                        "Nouvelle piste",
                        true,
                        Some("CmdOrCtrl+Shift+T"),
                    )?,
                    &delete_track_item,
                ],
            )?;

            #[cfg(target_os = "macos")]
            let menu = Menu::with_items(
                app,
                &[&app_menu, &file_menu, &edit_menu, &track_menu],
            )?;
            #[cfg(not(target_os = "macos"))]
            let menu = Menu::with_items(
                app,
                &[&file_menu, &edit_menu, &track_menu, &app_menu],
            )?;
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
            list_plugins,
            get_unavailable_plugins,
            get_plugin_params,
            scan_plugins,
            open_plugin_editor,
            close_plugin_editor,
            send_command,
            send_batch,
            set_track_mix,
            preview_midi_note,
            get_snapshot,
            get_graph_snapshot,
            undo,
            redo,
            menu_action,
            sync_menu_track_selection,
            export_wav,
            export_mp3,
            get_audio_devices,
            get_device_sample_rates,
            get_device_channels,
            apply_audio_settings,
            set_plugin_isolation,
            take_crash_suspect,
            reload_plugins,
            set_active_output_channels,
            sync_project_ui_layout,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_, event| {
            if let tauri::RunEvent::Exit = event {
                plugin_system::crash_marker::mark_clean_exit();
            }
        });
}

/// Plugin tags left running by a previous session that died (see `crash_marker`).
struct CrashSuspects(std::sync::Mutex<Vec<u64>>);

fn open_crash_marker(app: &tauri::App) -> Vec<u64> {
    let Ok(dir) = app.path().app_data_dir() else {
        return Vec::new();
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return Vec::new();
    }
    plugin_system::crash_marker::open(&dir.join("plugin-crash-marker.bin"))
}

/// Restart the plugins that crashed: a recompile rebuilds every instance in
/// the error state, from its last saved state.
#[tauri::command]
fn reload_plugins(app: tauri::AppHandle, state: tauri::State<'_, AppState>) {
    recompile_audio_graph(&app, &state);
}

/// A plugin that was running when Echoes last closed unexpectedly.
#[derive(serde::Serialize)]
struct CrashSuspect {
    plugin_id: String,
    name: String,
    /// Only CLAP plugins can run isolated so far.
    can_isolate: bool,
}

/// The plugin suspected of the previous crash, once per session.
#[tauri::command]
fn take_crash_suspect(
    suspects: tauri::State<'_, CrashSuspects>,
    state: tauri::State<'_, AppState>,
) -> Result<Option<CrashSuspect>, String> {
    let tags = std::mem::take(&mut *suspects.0.lock().map_err(|e| e.to_string())?);
    let engine = state.audio_engine.lock().map_err(|e| e.to_string())?;
    let suspect = engine
        .plugin_host
        .registry()
        .all()
        .find(|manifest| tags.contains(&plugin_system::crash_marker::plugin_tag(&manifest.id)))
        .map(|manifest| CrashSuspect {
            plugin_id: manifest.id.clone(),
            name: manifest.name.clone(),
            can_isolate: manifest.format == plugin_system::wrapper::DeviceFormat::Clap,
        });
    Ok(suspect)
}

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
    install_loaded_project(app, state, project, file_path);
    set_window_title(app, &name);
    let _ = app.emit("project-loaded", dto);
    menu_state::refresh_menu(app, state);
}

/// Engine side of opening a project — state, staged plugin presets, graph —
/// shared by the Open menu and the debug auto-load.
fn install_loaded_project(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    project: AppProject,
    file_path: std::path::PathBuf,
) {
    // Capture plugin state blobs before `project` is moved into AppState — they
    // are re-applied after the graph compiles (the instances must exist first).
    // Keyed per placement by `instance_key` (ADR-017).
    let plugin_states = project.plugin_states.clone();

    // Seed event store from loaded project (snapshot-at-zero approach)
    if let (Ok(mut store), Ok(mut p)) = (state.event_store.lock(), state.project.lock()) {
        store.init_from_project(engine_gateway::api::snapshot_project_for_store(&project));
        *p = project;
    }

    if let Ok(mut p) = state.project_path.lock() {
        *p = Some(file_path);
    }

    // Stage plugin state so the upcoming compile applies it *at instantiation*,
    // before any node enters a snapshot — rather than pushing `setState` onto the
    // already-snapshotted instances afterwards, which raced the audio thread on
    // the shared handle (ADR-010 race-free restore).
    if !plugin_states.is_empty() {
        if let Ok(mut engine) = state.audio_engine.lock() {
            engine.plugin_host.set_pending_state_blobs(plugin_states);
        }
    }

    // Recompile the audio graph so the engine knows about the loaded clips.
    // Without this the scheduler stays at the empty-project state and produces
    // silence even though the UI shows the correct tracks and clips.  The staged
    // plugin state is consumed here, during instantiation.
    recompile_audio_graph(app, state);

    // Drop the staged blobs: they have been applied; a later structural recompile
    // must not re-apply this loaded state over the instance's live state.
    if let Ok(mut engine) = state.audio_engine.lock() {
        engine.plugin_host.clear_pending_state_blobs();
    }

    mark_saved(state);
}

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
            recompile_audio_graph(&app, &state);
            mark_saved(&state);
            set_window_title(app, "Sans titre");
            let _ = app.emit("project-loaded", dto);
            menu_state::refresh_menu(app, &state);
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
                // Canonical lock order: project → engine (matches
                // `recompile_audio_graph` / `render_project_to_stereo`).
                let mut project = state.project.lock().unwrap();
                // Harvest live plugin state so the saved file carries the current
                // presets (Phase 4b / ADR-010).  Keyed per placement by
                // `instance_key` (ADR-017).  Off-RT; each live getState is fenced
                // by the instance FFI gate, so this is safe even during playback.
                if let Ok(engine) = state.audio_engine.lock() {
                    project.plugin_states = engine
                        .plugin_host
                        .collect_plugin_states()
                        .into_iter()
                        .collect();
                }
                serde_json::to_string_pretty(&project.to_file())
            }
            .and_then(|json| {
                std::fs::write(&save_path, json.as_bytes()).map_err(serde_json::Error::io)
            });

            if write_result.is_ok() {
                if let Ok(mut p) = state.project_path.lock() {
                    *p = Some(save_path.clone());
                }
                mark_saved(&state);
                let _ = app.emit("project-saved", save_path.to_string_lossy().to_string());
                menu_state::refresh_menu(app, &state);
            } else if let Err(e) = write_result {
                eprintln!("[projet] Échec sauvegarde : {e}");
            }
        }

        "import-audio" => {
            let picked = app
                .dialog()
                .file()
                .add_filter("Fichiers audio", &["wav", "mp3", "flac", "ogg", "oga"])
                .blocking_pick_file();

            if let Some(FilePath::Path(file_path)) = picked {
                let path_str = file_path.to_string_lossy().to_string();
                let _ = app.emit("audio-file-picked", path_str);
            }
        }

        "export-wav" => {
            let _ = app.emit("menu-export", "wav");
        }

        "export-mp3" => {
            let _ = app.emit("menu-export", "mp3");
        }

        "preferences" => {
            let _ = app.emit("open-preferences", ());
        }

        "project-settings" => {
            let _ = app.emit("open-project-settings", ());
        }

        "add-track" => {
            let _ = app.emit("track-action", "add-track");
        }

        "delete-track" => {
            let _ = app.emit("track-action", "delete-track");
        }

        _ => {} // actions inconnues ignorées silencieusement
    }
}

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

#[tauri::command]
fn sync_menu_track_selection(app: tauri::AppHandle, has_selected_track: bool) {
    menu_state::set_delete_track_enabled(&app, has_selected_track);
}

/// Duration of an audio file in seconds — used for drag-and-drop preview.
/// Supports WAV, MP3, FLAC, OGG via symphonia.
#[tauri::command]
fn get_audio_clip_duration(file_path: &str) -> Result<f64, String> {
    use std::path::Path;
    use symphonia::core::{
        codecs::DecoderOptions, errors::Error as SymphoniaError, formats::FormatOptions,
        io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
    };

    fn make_hint(file_path: &str) -> Hint {
        let mut hint = Hint::new();
        if let Some(ext) = Path::new(file_path).extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        hint
    }

    fn open_format(
        file_path: &str,
    ) -> Result<Box<dyn symphonia::core::formats::FormatReader>, String> {
        let file = std::fs::File::open(file_path).map_err(|e| e.to_string())?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        symphonia::default::get_probe()
            .format(
                &make_hint(file_path),
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map(|p| p.format)
            .map_err(|e| e.to_string())
    }

    let (sample_rate, time_base, n_frames, track_id) = {
        let format = open_format(file_path)?;
        let track = format.default_track().ok_or("No audio track found")?;
        (
            track
                .codec_params
                .sample_rate
                .ok_or("Unknown sample rate")?,
            track.codec_params.time_base,
            track.codec_params.n_frames,
            track.id,
        )
    };

    // Fast path: embedded duration (WAV, FLAC, OGG, CBR MP3 with Xing header)
    if let (Some(tb), Some(n)) = (time_base, n_frames) {
        let t = tb.calc_time(n);
        return Ok(t.seconds as f64 + t.frac);
    }

    // Slow path: full decode to count frames (VBR MP3 without Xing header)
    let mut format = open_format(file_path)?;
    let mut decoder = {
        let track = format.default_track().ok_or("No audio track found")?;
        symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| e.to_string())?
    };

    let mut total_frames: u64 = 0;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => total_frames += decoded.frames() as u64,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(_) => break,
        }
    }

    Ok(total_frames as f64 / sample_rate as f64)
}

/// Return the list of all discovered VST3 and CLAP plugins.
#[tauri::command]
fn list_plugins(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<plugin_system::host::PluginInfo>, String> {
    let engine = state.audio_engine.lock().map_err(|e| e.to_string())?;
    Ok(engine.plugin_host.list_plugins())
}

/// Re-scan plugin directories on demand (e.g. after installing a new plugin).
/// Recompiles afterwards so a plugin installed since the project was opened
/// loads right away instead of staying unavailable.
#[tauri::command]
fn scan_plugins(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<plugin_system::host::PluginInfo>, String> {
    let list = {
        let mut engine = state.audio_engine.lock().map_err(|e| e.to_string())?;
        engine.plugin_host.scan_plugins();
        engine.plugin_host.list_plugins()
    };
    recompile_audio_graph(&app, &state);
    Ok(list)
}

/// Parameters of a loaded plugin placement (`effect.id` or `instrument:{track_id}`).
#[tauri::command]
fn get_plugin_params(
    state: tauri::State<'_, AppState>,
    instance_key: String,
) -> Result<Vec<plugin_system::parameter::PluginParamInfo>, String> {
    let engine = state.audio_engine.lock().map_err(|e| e.to_string())?;
    engine
        .plugin_host
        .plugin_params(&instance_key)
        .map_err(|e| e.to_string())
}

/// Which plugins run in a sandbox process (app preference, ADR-015).  A change
/// recompiles, which moves the affected plugins in or out of their sandbox.
#[tauri::command]
fn set_plugin_isolation(
    mode: plugin_system::host::PluginIsolation,
    always_isolated: std::collections::HashSet<String>,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let changed = state
        .audio_engine
        .lock()
        .map_err(|e| e.to_string())?
        .plugin_host
        .set_isolation(mode, always_isolated);
    if changed {
        // An open editor would stay attached to the instance the recompile replaces.
        if let Ok(editors) = state.open_plugin_editors.lock() {
            editors.values().for_each(|&hwnd| plugin_system::gui::post_close(hwnd));
        }
        recompile_audio_graph(&app, &state);
    }
    Ok(())
}

/// Placements whose plugin could not be loaded; updates arrive through the
/// `plugin-availability` event after every recompile.
#[tauri::command]
fn get_unavailable_plugins(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<plugin_system::host::UnavailablePlugin>, String> {
    let engine = state.audio_engine.lock().map_err(|e| e.to_string())?;
    Ok(engine.plugin_host.unavailable_plugins())
}

/// Full project snapshot for initial frontend hydration.
/// All subsequent mutations go through `send_command`.
#[tauri::command]
fn get_project(state: tauri::State<'_, AppState>) -> Result<ProjectDto, String> {
    let project = state.project.lock().map_err(|e| e.to_string())?;
    Ok(project.to_dto())
}

/// Met à jour la disposition des panneaux (hors event store / undo).
#[tauri::command]
fn sync_project_ui_layout(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    ui_layout: ProjectUiLayout,
) -> Result<(), String> {
    let mut project = state.project.lock().map_err(|e| e.to_string())?;
    project.ui_layout = ui_layout;
    state
        .dirty
        .store(true, std::sync::atomic::Ordering::Relaxed);
    menu_state::refresh_menu(&app, &state);
    Ok(())
}

/// Render the current project offline through the real Rust audio engine.
///
/// Returns interleaved-channel-split f32 buffers `(left, right)`.  Lives on
/// the command thread; never touches the live CPAL stream.  Reflects current
/// `LiveParams` (volume, mute/solo, modulation depth, …) and the engine's
/// `PluginHost` so the export is bit-identical to what is being heard.
fn render_project_to_stereo(
    state: &tauri::State<'_, AppState>,
    sample_rate: u32,
) -> Result<(Vec<f32>, Vec<f32>), String> {
    let project = state.project.lock().map_err(|e| e.to_string())?;
    let mut engine = state.audio_engine.lock().map_err(|e| e.to_string())?;
    audio_engine::offline::render_project_offline(
        &project,
        &state.live_params,
        &mut engine.plugin_host,
        sample_rate,
    )
}

/// Write a stereo mixdown as a WAV file.  `bit_depth` may be 16, 24, or 32.
#[tauri::command]
fn export_wav(
    state: tauri::State<'_, AppState>,
    path: String,
    sample_rate: u32,
    bit_depth: u8,
) -> Result<(), String> {
    let (samples_left, samples_right) = render_project_to_stereo(&state, sample_rate)?;

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

/// Encode a stereo mixdown as an MP3 file using LAME.  `bitrate_kbps` is
/// typically 128, 192, or 320.
#[tauri::command]
fn export_mp3(
    state: tauri::State<'_, AppState>,
    path: String,
    sample_rate: u32,
    bitrate_kbps: u32,
) -> Result<(), String> {
    use mp3lame_encoder::{Builder, DualPcm, FlushNoGap};

    let (samples_left, samples_right) = render_project_to_stereo(&state, sample_rate)?;

    let path = std::path::PathBuf::from(&path);
    let path = if path.extension().map_or(true, |e| e != "mp3") {
        path.with_extension("mp3")
    } else {
        path
    };

    let left_i16: Vec<i16> = samples_left
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * 32_767.0) as i16)
        .collect();
    let right_i16: Vec<i16> = samples_right
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * 32_767.0) as i16)
        .collect();

    let mut builder = Builder::new().ok_or("Impossible de créer l'encodeur LAME")?;
    builder
        .set_num_channels(2)
        .map_err(|e| format!("set_num_channels: {e:?}"))?;
    builder
        .set_sample_rate(sample_rate)
        .map_err(|e| format!("set_sample_rate: {e:?}"))?;
    builder
        .set_brate(match bitrate_kbps {
            128 => mp3lame_encoder::Birtate::Kbps128,
            192 => mp3lame_encoder::Birtate::Kbps192,
            _ => mp3lame_encoder::Birtate::Kbps320,
        })
        .map_err(|e| format!("set_brate: {e:?}"))?;
    builder
        .set_quality(mp3lame_encoder::Quality::Best)
        .map_err(|e| format!("set_quality: {e:?}"))?;
    let mut encoder = builder
        .build()
        .map_err(|e| format!("Impossible d'initialiser l'encodeur LAME : {e:?}"))?;

    let input = DualPcm {
        left: &left_i16,
        right: &right_i16,
    };

    let mut mp3_out: Vec<u8> = Vec::new();
    mp3_out.reserve(mp3lame_encoder::max_required_buffer_size(left_i16.len()));

    let encoded_size = encoder
        .encode(input, mp3_out.spare_capacity_mut())
        .map_err(|e| format!("Erreur encodage MP3 : {e:?}"))?;
    // SAFETY: `encode` filled exactly `encoded_size` bytes into the spare capacity.
    unsafe { mp3_out.set_len(mp3_out.len() + encoded_size) };

    let flush_size = encoder
        .flush::<FlushNoGap>(mp3_out.spare_capacity_mut())
        .map_err(|e| format!("Erreur flush MP3 : {e:?}"))?;
    // SAFETY: `flush` filled exactly `flush_size` bytes.
    unsafe { mp3_out.set_len(mp3_out.len() + flush_size) };

    std::fs::write(&path, &mp3_out).map_err(|e| e.to_string())
}

/// Open the native GUI window for a plugin effect.
/// Idempotent: returns immediately if the editor is already open.
#[tauri::command]
fn open_plugin_editor(
    plugin_id: String,
    effect_id: String,
    plugin_name: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    if let Ok(editors) = state.open_plugin_editors.lock() {
        if editors.contains_key(&effect_id) {
            return Ok(());
        }
    }

    let (instance, format) = {
        let engine = state.audio_engine.lock().map_err(|e| e.to_string())?;
        let format = engine
            .plugin_host
            .registry()
            .get(&plugin_id)
            .map(|m| m.format)
            .ok_or_else(|| format!("plugin not in registry: {plugin_id}"))?;
        let inst = engine
            .plugin_host
            .instance_by_key(&effect_id)
            .or_else(|| engine.plugin_host.instance(&plugin_id))
            .ok_or_else(|| {
                format!(
                    "plugin instance not loaded: {plugin_id} (key={effect_id}, graph compiled?)"
                )
            })?;
        (inst, format)
    };

    // A sandboxed plugin's editor lives in its bridge process, which closes it
    // itself and takes it down with the process when the effect goes away.
    if let Some(bridged) = instance.bridge() {
        return bridged.open_editor(&plugin_name);
    }

    let open_editors = std::sync::Arc::clone(&state.open_plugin_editors);

    // IPlugView/clap_gui and Win32 window creation must happen on the main thread.
    let (tx, rx) = std::sync::mpsc::channel::<Result<usize, String>>();

    app.run_on_main_thread(move || {
        let result: Result<usize, String> = match format {
            #[cfg(windows)]
            plugin_system::wrapper::DeviceFormat::Vst3 => unsafe {
                plugin_system::gui::open_vst3_editor(
                    instance,
                    &plugin_name,
                    &effect_id,
                    open_editors,
                )
            },
            #[cfg(windows)]
            plugin_system::wrapper::DeviceFormat::Clap => unsafe {
                plugin_system::gui::open_clap_editor(
                    instance,
                    &plugin_name,
                    &effect_id,
                    open_editors,
                )
            },
            _ => Err("native GUI is only available on Windows".into()),
        };
        tx.send(result).ok();
    })
    .map_err(|e| e.to_string())?;

    rx.recv().map_err(|e| e.to_string())??;
    Ok(())
}

/// Programmatically close the native GUI window for a plugin effect.
#[tauri::command]
fn close_plugin_editor(effect_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let hwnd = state
        .open_plugin_editors
        .lock()
        .map_err(|e| e.to_string())?
        .get(&effect_id)
        .copied();

    if let Some(hwnd) = hwnd {
        plugin_system::gui::post_close(hwnd);
    }

    Ok(())
}

/// Start the CPAL audio engine during Tauri `setup`.
///
/// Extracted into a function so the compiler can track the `tauri::State`
/// lifetime — the `MutexGuard` temporary must not outlive `State`.
/// Edits reported by plugins up to now are part of the loaded or saved project.
fn mark_saved(state: &tauri::State<'_, AppState>) {
    plugin_formats::clap::ffi::take_plugin_state_edited();
    state.dirty.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Edits made inside a plugin window never go through `send_command`, so the
/// unsaved-changes flag is raised from the plugins' own reports.  Sandboxed
/// plugins that crash are reported the same way, without waiting for a recompile.
fn watch_plugin_edits(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut crashes_seen = plugin_system::crash_marker::crash_count();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let state = app.state::<AppState>();
            if plugin_formats::clap::ffi::take_plugin_state_edited() {
                state.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
                menu_state::refresh_menu(&app, &state);
            }
            let crashes = plugin_system::crash_marker::crash_count();
            if crashes != crashes_seen {
                crashes_seen = crashes;
                if let Ok(engine) = state.audio_engine.lock() {
                    let unavailable = engine.plugin_host.unavailable_plugins();
                    let _ = app.emit(engine_gateway::api::PLUGIN_AVAILABILITY_EVENT, unavailable);
                }
            }
        }
    });
}

fn start_native_audio(app: &mut tauri::App) {
    let app_handle = app.handle().clone();
    let state = app.state::<AppState>();

    let settings = state
        .audio_settings
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();

    let init_project = AppProject::new();

    if let Ok(mut engine) = state.audio_engine.lock() {
        if let Err(e) = engine.start_stream_with_settings(&init_project, app_handle, &settings) {
            eprintln!("[audio] Impossible d'activer le moteur natif : {e}");
        }
    };
}


/// Return available input and output audio devices.
/// `"default"` always resolves to the system default device.
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

/// Return which of [44100, 48000, 88200, 96000] Hz are supported by the device.
/// Falls back to all four candidates when the device cannot be queried.
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
            .and_then(|mut it| it.find(|d| d.name().map(|n| n == device_id).unwrap_or(false)))
            .or_else(|| host.default_output_device())
    };

    let Some(device) = device else {
        return CANDIDATES.to_vec();
    };

    let Ok(ranges) = device.supported_output_configs() else {
        return CANDIDATES.to_vec();
    };

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

/// Return the native channel count of a device.  Falls back to 2 on error.
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
                .and_then(|mut it| it.find(|d| d.name().map(|n| n == device_id).unwrap_or(false)))
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
                .and_then(|mut it| it.find(|d| d.name().map(|n| n == device_id).unwrap_or(false)))
                .or_else(|| host.default_output_device())
        };
        device
            .and_then(|d| d.default_output_config().ok())
            .map(|c| c.channels())
            .unwrap_or(2)
    }
}

/// Apply new audio settings: persist them and restart the CPAL stream only
/// when the device, sample rate, or buffer size changed.  Output channel count
/// is handled atomically without a restart.
#[tauri::command]
fn apply_audio_settings(
    settings: audio_settings::AudioSettings,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let current = state
        .audio_settings
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();

    if let Ok(mut s) = state.audio_settings.lock() {
        *s = settings.clone();
    }

    state
        .live_params
        .set_master_soft_clip(settings.master_soft_clip);

    let needs_restart = current.output_device != settings.output_device
        || current.input_device != settings.input_device
        || current.sample_rate != settings.sample_rate
        || current.buffer_size != settings.buffer_size;

    if needs_restart {
        let init_project = AppProject::new();
        if let Ok(mut engine) = state.audio_engine.lock() {
            engine.start_stream_with_settings(&init_project, app.clone(), &settings)?;
        }
        recompile_audio_graph(&app, &state);
    } else {
        if let Ok(engine) = state.audio_engine.lock() {
            engine.set_active_output_channels(settings.output_channels as usize);
        }
    }

    Ok(())
}

/// Update the active output channel count without restarting the CPAL stream.
/// `count = 0` restores the "use all hardware channels" default.
#[tauri::command]
fn set_active_output_channels(count: u16, state: tauri::State<'_, AppState>) {
    if let Ok(engine) = state.audio_engine.lock() {
        engine.set_active_output_channels(count as usize);
    }
}

/// Recompile the audio graph and push the new scheduler to the audio thread.
/// Must be called outside any held Mutex locks (canonical lock order: project → engine).
fn recompile_audio_graph(app: &tauri::AppHandle, state: &tauri::State<'_, AppState>) {
    engine_gateway::api::recompile_if_needed(app, state);
}
