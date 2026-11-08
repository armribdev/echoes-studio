//! Native application menu (File / Edit / Track) with EN/FR labels.
//!
//! Labels are updated in place via `MenuItem::set_text` / `Submenu::set_text`
//! so enable/disable handles stay valid across locale switches.

use std::collections::HashMap;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager};

use crate::menu_state::MenuHandles;

pub struct MenuLabels {
    pub file: &'static str,
    pub edit: &'static str,
    pub track: &'static str,
    pub app: &'static str,
    pub new_project: &'static str,
    pub open: &'static str,
    pub project_settings: &'static str,
    pub import_audio: &'static str,
    pub export: &'static str,
    pub export_wav: &'static str,
    pub export_mp3: &'static str,
    pub save: &'static str,
    pub save_as: &'static str,
    pub undo: &'static str,
    pub redo: &'static str,
    pub preferences: &'static str,
    pub add_track: &'static str,
    pub delete_track: &'static str,
}

pub fn labels_for(locale: &str) -> MenuLabels {
    if locale.eq_ignore_ascii_case("fr") {
        MenuLabels {
            file: "Fichier",
            edit: "Édition",
            track: "Piste",
            app: "Echoes Studio",
            new_project: "Nouveau projet",
            open: "Ouvrir...",
            project_settings: "Projet...",
            import_audio: "Importer audio...",
            export: "Exporter",
            export_wav: "WAV",
            export_mp3: "MP3",
            save: "Enregistrer",
            save_as: "Enregistrer sous...",
            undo: "Annuler",
            redo: "Rétablir",
            preferences: "Préférences...",
            add_track: "Nouvelle piste",
            delete_track: "Supprimer la piste sélectionnée",
        }
    } else {
        MenuLabels {
            file: "File",
            edit: "Edit",
            track: "Track",
            app: "Echoes Studio",
            new_project: "New Project",
            open: "Open...",
            project_settings: "Project...",
            import_audio: "Import Audio...",
            export: "Export",
            export_wav: "WAV",
            export_mp3: "MP3",
            save: "Save",
            save_as: "Save As...",
            undo: "Undo",
            redo: "Redo",
            preferences: "Preferences...",
            add_track: "New Track",
            delete_track: "Delete Selected Track",
        }
    }
}

struct LocaleItems {
    file_menu: Submenu<tauri::Wry>,
    edit_menu: Submenu<tauri::Wry>,
    track_menu: Submenu<tauri::Wry>,
    app_menu: Submenu<tauri::Wry>,
    export_menu: Submenu<tauri::Wry>,
    save: MenuItem<tauri::Wry>,
    undo: MenuItem<tauri::Wry>,
    redo: MenuItem<tauri::Wry>,
    delete_track: MenuItem<tauri::Wry>,
    new_project: MenuItem<tauri::Wry>,
    open: MenuItem<tauri::Wry>,
    project_settings: MenuItem<tauri::Wry>,
    import_audio: MenuItem<tauri::Wry>,
    export_wav: MenuItem<tauri::Wry>,
    export_mp3: MenuItem<tauri::Wry>,
    save_as: MenuItem<tauri::Wry>,
    preferences: MenuItem<tauri::Wry>,
    add_track: MenuItem<tauri::Wry>,
}

fn apply_labels(items: &LocaleItems, labels: &MenuLabels) -> tauri::Result<()> {
    items.file_menu.set_text(labels.file)?;
    items.edit_menu.set_text(labels.edit)?;
    items.track_menu.set_text(labels.track)?;
    items.app_menu.set_text(labels.app)?;
    items.export_menu.set_text(labels.export)?;
    items.save.set_text(labels.save)?;
    items.undo.set_text(labels.undo)?;
    items.redo.set_text(labels.redo)?;
    items.delete_track.set_text(labels.delete_track)?;
    items.new_project.set_text(labels.new_project)?;
    items.open.set_text(labels.open)?;
    items.project_settings.set_text(labels.project_settings)?;
    items.import_audio.set_text(labels.import_audio)?;
    items.export_wav.set_text(labels.export_wav)?;
    items.export_mp3.set_text(labels.export_mp3)?;
    items.save_as.set_text(labels.save_as)?;
    items.preferences.set_text(labels.preferences)?;
    items.add_track.set_text(labels.add_track)?;
    Ok(())
}

pub fn build_app_menu(
    app: &AppHandle,
    locale: &str,
) -> tauri::Result<(Menu<tauri::Wry>, MenuHandles)> {
    let labels = labels_for(locale);

    let save_item = MenuItem::with_id(app, "save", labels.save, false, Some("CmdOrCtrl+S"))?;
    let undo_item = MenuItem::with_id(app, "undo", labels.undo, false, Some("CmdOrCtrl+Z"))?;
    let redo_item = MenuItem::with_id(app, "redo", labels.redo, false, Some("CmdOrCtrl+Shift+Z"))?;
    let delete_track_item = MenuItem::with_id(
        app,
        "delete-track",
        labels.delete_track,
        false,
        None::<&str>,
    )?;
    let new_project = MenuItem::with_id(app, "new", labels.new_project, true, Some("CmdOrCtrl+N"))?;
    let open = MenuItem::with_id(app, "open", labels.open, true, Some("CmdOrCtrl+O"))?;
    let project_settings = MenuItem::with_id(
        app,
        "project-settings",
        labels.project_settings,
        true,
        Some("CmdOrCtrl+Shift+,"),
    )?;
    let import_audio =
        MenuItem::with_id(app, "import-audio", labels.import_audio, true, None::<&str>)?;
    let export_wav = MenuItem::with_id(
        app,
        "export-wav",
        labels.export_wav,
        true,
        Some("CmdOrCtrl+Shift+E"),
    )?;
    let export_mp3 = MenuItem::with_id(
        app,
        "export-mp3",
        labels.export_mp3,
        true,
        Some("CmdOrCtrl+Shift+M"),
    )?;
    let save_as = MenuItem::with_id(app, "save-as", labels.save_as, true, None::<&str>)?;
    let preferences = MenuItem::with_id(
        app,
        "preferences",
        labels.preferences,
        true,
        Some("CmdOrCtrl+,"),
    )?;
    let add_track = MenuItem::with_id(
        app,
        "add-track",
        labels.add_track,
        true,
        Some("CmdOrCtrl+Shift+T"),
    )?;

    let export_menu = Submenu::with_items(app, labels.export, true, &[&export_wav, &export_mp3])?;

    let file_menu = Submenu::with_items(
        app,
        labels.file,
        true,
        &[
            &new_project,
            &open,
            &project_settings,
            &PredefinedMenuItem::separator(app)?,
            &import_audio,
            &PredefinedMenuItem::separator(app)?,
            &export_menu,
            &PredefinedMenuItem::separator(app)?,
            &save_item,
            &save_as,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let edit_menu = Submenu::with_items(app, labels.edit, true, &[&undo_item, &redo_item])?;
    let app_menu = Submenu::with_items(app, labels.app, true, &[&preferences])?;
    let track_menu =
        Submenu::with_items(app, labels.track, true, &[&add_track, &delete_track_item])?;

    #[cfg(target_os = "macos")]
    let menu = Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu, &track_menu])?;
    #[cfg(not(target_os = "macos"))]
    let menu = Menu::with_items(app, &[&file_menu, &edit_menu, &track_menu, &app_menu])?;

    let locale_items = LocaleItems {
        file_menu: file_menu.clone(),
        edit_menu: edit_menu.clone(),
        track_menu: track_menu.clone(),
        app_menu: app_menu.clone(),
        export_menu: export_menu.clone(),
        save: save_item.clone(),
        undo: undo_item.clone(),
        redo: redo_item.clone(),
        delete_track: delete_track_item.clone(),
        new_project: new_project.clone(),
        open: open.clone(),
        project_settings: project_settings.clone(),
        import_audio: import_audio.clone(),
        export_wav: export_wav.clone(),
        export_mp3: export_mp3.clone(),
        save_as: save_as.clone(),
        preferences: preferences.clone(),
        add_track: add_track.clone(),
    };

    let save_h = save_item.clone();
    let undo_h = undo_item.clone();
    let redo_h = redo_item.clone();
    let delete_track_h = delete_track_item.clone();

    let accel_new = new_project.clone();
    let accel_open = open.clone();
    let accel_save = save_item.clone();
    let accel_save_as = save_as.clone();
    let accel_import = import_audio.clone();
    let accel_export_wav = export_wav.clone();
    let accel_export_mp3 = export_mp3.clone();
    let accel_undo = undo_item.clone();
    let accel_redo = redo_item.clone();
    let accel_add_track = add_track.clone();
    let accel_prefs = preferences.clone();
    let accel_project = project_settings.clone();

    let handles = MenuHandles {
        set_undo: Box::new(move |e| undo_h.set_enabled(e)),
        set_redo: Box::new(move |e| redo_h.set_enabled(e)),
        set_save: Box::new(move |e| save_h.set_enabled(e)),
        set_delete_track: Box::new(move |e| delete_track_h.set_enabled(e)),
        apply_locale: Box::new(move |locale| apply_labels(&locale_items, &labels_for(locale))),
        apply_accelerators: Box::new(move |map: &HashMap<String, String>| {
            let set = |item: &MenuItem<tauri::Wry>, id: &str| -> tauri::Result<()> {
                if let Some(accel) = map.get(id) {
                    item.set_accelerator(Some(accel.as_str()))?;
                }
                Ok(())
            };
            set(&accel_new, "new")?;
            set(&accel_open, "open")?;
            set(&accel_save, "save")?;
            set(&accel_save_as, "save-as")?;
            set(&accel_import, "import-audio")?;
            set(&accel_export_wav, "export-wav")?;
            set(&accel_export_mp3, "export-mp3")?;
            set(&accel_undo, "undo")?;
            set(&accel_redo, "redo")?;
            set(&accel_add_track, "add-track")?;
            set(&accel_prefs, "preferences")?;
            set(&accel_project, "project-settings")?;
            Ok(())
        }),
    };

    Ok((menu, handles))
}

#[tauri::command]
pub fn set_ui_locale(app: AppHandle, locale: String) -> Result<(), String> {
    let handles = app.try_state::<MenuHandles>().ok_or("menu not ready")?;
    (handles.apply_locale)(&locale).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_menu_accelerators(
    app: AppHandle,
    accelerators: HashMap<String, String>,
) -> Result<(), String> {
    let handles = app.try_state::<MenuHandles>().ok_or("menu not ready")?;
    (handles.apply_accelerators)(&accelerators).map_err(|e| e.to_string())
}

/// Historical default until the SPA syncs the saved preference.
pub fn locale_from_preferences_storage() -> String {
    "fr".to_string()
}
