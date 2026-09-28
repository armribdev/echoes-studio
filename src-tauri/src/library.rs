//! Async folder indexer for the content library (ADR-023 Phase 1).
//! Runs off the audio callback — scan work is background-only.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

const AUDIO_EXTS: &[&str] = &["wav", "mp3", "flac", "ogg", "oga"];
const BATCH_SIZE: usize = 64;
const MAX_DEPTH: usize = 12;
const MAX_FILES: usize = 50_000;

#[derive(Clone, Serialize)]
pub struct LibraryScanEntry {
    pub path: String,
    pub name: String,
    pub extension: String,
}

#[derive(Clone, Serialize)]
struct ScanBatchPayload {
    token: u64,
    entries: Vec<LibraryScanEntry>,
    /// 0..1 rough progress (files discovered / cap).
    progress: f64,
}

#[derive(Clone, Serialize)]
struct ScanDonePayload {
    token: u64,
    total: u64,
}

#[derive(Clone, Serialize)]
struct ScanErrorPayload {
    token: u64,
    message: String,
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.iter().any(|ok| e.eq_ignore_ascii_case(ok)))
        .unwrap_or(false)
}

fn collect_audio_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];

    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                if dir == root {
                    return Err(format!("Cannot read folder: {e}"));
                }
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push((path, depth + 1));
            } else if ft.is_file() && is_audio_file(&path) {
                out.push(path);
                if out.len() >= MAX_FILES {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

fn entry_from_path(path: &Path) -> Option<LibraryScanEntry> {
    let path_str = path.to_string_lossy().to_string();
    let name = path.file_name()?.to_string_lossy().to_string();
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    Some(LibraryScanEntry {
        path: path_str,
        name,
        extension,
    })
}

/// Kick off a non-blocking recursive scan. Emits `library://scan-batch`,
/// then `library://scan-done` (or `library://scan-error`).
#[tauri::command]
pub fn scan_library_folder(app: AppHandle, folder_path: String, token: u64) -> Result<(), String> {
    let root = PathBuf::from(&folder_path);
    if !root.is_dir() {
        return Err(format!("Not a folder: {folder_path}"));
    }

    std::thread::spawn(move || {
        let files = match collect_audio_files(&root) {
            Ok(f) => f,
            Err(message) => {
                let _ = app.emit(
                    "library://scan-error",
                    ScanErrorPayload { token, message },
                );
                return;
            }
        };

        let total = files.len() as u64;
        if total == 0 {
            let _ = app.emit("library://scan-done", ScanDonePayload { token, total: 0 });
            return;
        }

        let mut batch = Vec::with_capacity(BATCH_SIZE);
        for (i, path) in files.iter().enumerate() {
            if let Some(entry) = entry_from_path(path) {
                batch.push(entry);
            }
            if batch.len() >= BATCH_SIZE || i + 1 == files.len() {
                let progress = (i + 1) as f64 / total as f64;
                let _ = app.emit(
                    "library://scan-batch",
                    ScanBatchPayload {
                        token,
                        entries: std::mem::take(&mut batch),
                        progress,
                    },
                );
            }
        }

        let _ = app.emit("library://scan-done", ScanDonePayload { token, total });
    });

    Ok(())
}
