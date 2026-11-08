//! Native menu state synchronisation.
//!
//! `MenuHandles` stores direct update closures for controlled menu items.
//! This avoids the unreliable `app.menu().get(id)` lookup which can return
//! `None` on Windows when items are nested in a submenu or when the app-level
//! menu is not yet fully initialised.
//!
//! Usage:
//!   1. Create the `MenuItem`s with `enabled: false` where needed.
//!   2. Build `MenuHandles` from them and call `app.manage()`.
//!   3. Call `refresh_menu` / `set_delete_track_enabled` after state changes.
//!   4. Call `apply_locale` when the UI language changes.
//!   5. Call `apply_accelerators` when shortcut preferences change.

use std::collections::HashMap;

use crate::project::AppState;
use tauri::Manager;

type LocaleApplier = Box<dyn Fn(&str) -> tauri::Result<()> + Send + Sync>;
type AcceleratorApplier = Box<dyn Fn(&HashMap<String, String>) -> tauri::Result<()> + Send + Sync>;

/// Holds `set_enabled` / `set_text` / `set_accelerator` closures for controlled menu items.
pub struct MenuHandles {
    pub set_undo: Box<dyn Fn(bool) -> tauri::Result<()> + Send + Sync>,
    pub set_redo: Box<dyn Fn(bool) -> tauri::Result<()> + Send + Sync>,
    pub set_save: Box<dyn Fn(bool) -> tauri::Result<()> + Send + Sync>,
    pub set_delete_track: Box<dyn Fn(bool) -> tauri::Result<()> + Send + Sync>,
    pub apply_locale: LocaleApplier,
    pub apply_accelerators: AcceleratorApplier,
}

pub fn refresh_menu(app: &tauri::AppHandle, state: &tauri::State<'_, AppState>) {
    let (can_undo, can_redo) = state
        .event_store
        .lock()
        .map(|s| (s.can_undo(), s.can_redo()))
        .unwrap_or((false, false));

    let is_dirty = state.dirty.load(std::sync::atomic::Ordering::Relaxed);

    let handles = app.state::<MenuHandles>();
    let _ = (handles.set_undo)(can_undo);
    let _ = (handles.set_redo)(can_redo);
    let _ = (handles.set_save)(is_dirty);
}

pub fn set_delete_track_enabled(app: &tauri::AppHandle, enabled: bool) {
    let handles = app.state::<MenuHandles>();
    let _ = (handles.set_delete_track)(enabled);
}
