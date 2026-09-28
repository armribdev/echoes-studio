/**
 * Keyboard shortcut manager — single registration point for the app.
 *
 * Architecture:
 * - Registered once in the root +page.svelte via onMount → cleanup on destroy.
 * - File operations (new/open/save) go through the `menu_action` Tauri command,
 *   which reuses the exact same Rust logic as the native menu items.
 * - History operations (undo/redo) call the existing dispatcher helpers.
 * - Shortcuts are intentionally suppressed when the user is typing in an input.
 *
 * To add a shortcut: append an entry to SHORTCUTS below.
 * To expose a new action: add a `#[tauri::command] fn menu_action` branch in main.rs.
 */

import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { undoCommand, redoCommand, play, pause } from '../commands/dispatcher';
import { transport } from '../../stores/transport';
import { notify } from '../../stores/status';
import { isEditingText, shouldHandleGlobalHistoryShortcuts, hasNoModifiers } from './context';
import { engineProject } from '../store/engineStore';
import { selection } from '../../stores/selection';
import { bottomPanel } from '../../stores/bottomPanel';
import { settingsPanelOpen } from '../../stores/preferences';
import { cycleEditorFocus, handleEditorEscapeKey } from '../pianoRoll/clipEditor';
import { viewMode } from '../../stores/viewMode';
import { dismissTopLayer } from '../dismiss';
import { rightPanel } from '../../stores/rightPanel';
import { keyboardFocus } from '../../stores/keyboardFocus';
import { libraryStore, requestLibrarySearchFocus } from '../library';

// ── Types ─────────────────────────────────────────────────────────────────────

type Modifier = 'ctrl' | 'shift' | 'alt';

interface Shortcut {
  /** Single key (case-insensitive). */
  key: string;
  /** Required modifiers — all must be held, no extras. */
  modifiers: Modifier[];
  /** Human-readable label — for a future shortcut-help panel. */
  description: string;
  action: () => void;
}

// ── Shortcut table ────────────────────────────────────────────────────────────
//
// Mirror every accelerator defined in main.rs ↔ Submenu::with_items.

const menuAction = (action: string) => () =>
  invoke('menu_action', { action }).catch(console.error);

const SHORTCUTS: Shortcut[] = [
  // ── File ──────────────────────────────────────────────────────────────────
  {
    key: 'n',
    modifiers: ['ctrl'],
    description: 'Nouveau projet',
    action: menuAction('new'),
  },
  {
    key: 'o',
    modifiers: ['ctrl'],
    description: 'Ouvrir…',
    action: menuAction('open'),
  },
  {
    key: 's',
    modifiers: ['ctrl'],
    description: 'Enregistrer',
    action: menuAction('save'),
  },
  {
    key: 's',
    modifiers: ['ctrl', 'shift'],
    description: 'Enregistrer sous…',
    action: menuAction('save-as'),
  },

  // ── History ───────────────────────────────────────────────────────────────
  {
    key: 'z',
    modifiers: ['ctrl'],
    description: 'Annuler',
    action: () => undoCommand().catch((e) => notify(`Annuler : ${e?.message ?? e}`, 'error')),
  },
  {
    key: 'z',
    modifiers: ['ctrl', 'shift'],
    description: 'Rétablir',
    action: () => redoCommand().catch((e) => notify(`Rétablir : ${e?.message ?? e}`, 'error')),
  },
  {
    key: 'y',
    modifiers: ['ctrl'],
    description: 'Rétablir (Windows)',
    action: () => redoCommand().catch((e) => notify(`Rétablir : ${e?.message ?? e}`, 'error')),
  },

  // ── Import / Export ───────────────────────────────────────────────────────
  {
    key: 'i',
    modifiers: ['ctrl', 'shift'],
    description: 'Importer audio…',
    action: menuAction('import-audio'),
  },
  {
    key: 'e',
    modifiers: ['ctrl', 'shift'],
    description: 'Exporter mixdown WAV…',
    action: menuAction('export-wav'),
  },
  {
    key: 'm',
    modifiers: ['ctrl', 'shift'],
    description: 'Exporter mixdown MP3…',
    action: menuAction('export-mp3'),
  },

  // ── Transport ─────────────────────────────────────────────────────────────
  {
    key: ' ',
    modifiers: [],
    description: 'Lecture / Pause (ou aperçu bibliothèque)',
    action: () => {
      if (get(keyboardFocus) === 'library-results') {
        const selected = libraryStore.getSelectedAsset();
        if (selected) {
          void libraryStore.togglePreview(selected.id).catch((e) =>
            notify(`Aperçu : ${e?.message ?? e}`, 'error'),
          );
          return;
        }
      }
      const { isPlaying } = get(transport);
      (isPlaying ? pause() : play()).catch(console.error);
    },
  },

  // ── Library ───────────────────────────────────────────────────────────────
  {
    key: 'b',
    modifiers: [],
    description: 'Basculer la bibliothèque',
    action: () => rightPanel.toggle(),
  },
  {
    key: 'l',
    modifiers: [],
    description: 'Basculer la bibliothèque (alias)',
    action: () => rightPanel.toggle(),
  },
  {
    key: '/',
    modifiers: [],
    description: 'Focus recherche bibliothèque',
    action: () => {
      if (!get(rightPanel).open) rightPanel.toggle();
      requestLibrarySearchFocus();
    },
  },
  {
    key: 'f',
    modifiers: ['ctrl'],
    description: 'Focus recherche bibliothèque',
    action: () => {
      if (!get(rightPanel).open) return;
      requestLibrarySearchFocus();
    },
  },

  // ── View ──────────────────────────────────────────────────────────────────
  {
    key: 'g',
    modifiers: [],
    description: 'Basculer Arrangement / Graph',
    action: () => viewMode.toggleMain(),
  },

  // ── Editor panel ──────────────────────────────────────────────────────────
  {
    key: 'Tab',
    modifiers: [],
    description: 'Plonger dans l’éditeur MIDI / remonter au focus arrangement',
    action: () => cycleEditorFocus(get(engineProject).tracks, get(selection)),
  },
];

// ── Matching logic ────────────────────────────────────────────────────────────

/**
 * True when the event exactly satisfies a shortcut's key + modifiers.
 *
 * Note: `ctrl` in the modifier list maps to Cmd on macOS (e.metaKey) and
 * Ctrl on Windows/Linux (e.ctrlKey) — both are accepted via `ctrlHeld`.
 */
function matches(e: KeyboardEvent, shortcut: Shortcut): boolean {
  if (e.key.toLowerCase() !== shortcut.key.toLowerCase()) return false;

  const needs = shortcut.modifiers;

  // On macOS, Cmd replaces Ctrl; on Windows/Linux, only Ctrl is used.
  const ctrlHeld = e.ctrlKey || e.metaKey;

  if (needs.includes('ctrl') !== ctrlHeld) return false;
  if (needs.includes('shift') !== e.shiftKey) return false;
  if (needs.includes('alt') !== e.altKey) return false;

  return true;
}

// ── Public API ────────────────────────────────────────────────────────────────

/**
 * Attach the global keyboard handler to `window`.
 *
 * @returns A cleanup function — pass it directly as the onMount return value.
 *
 * @example
 * onMount(() => registerShortcuts());
 */
export function registerShortcuts(): () => void {
  const handler = (e: KeyboardEvent): void => {
    if (e.repeat) return;

    if (e.key === 'Escape' && dismissTopLayer()) {
      e.preventDefault();
      e.stopImmediatePropagation();
      return;
    }

    if (isEditingText(e.target)) {
      // ADR-023: B/L still toggle the library from the search field.
      if (hasNoModifiers(e) && !e.shiftKey) {
        const k = e.key.toLowerCase();
        if (k === 'b' || k === 'l') {
          e.preventDefault();
          rightPanel.toggle();
          return;
        }
      }
      return;
    }

    // Piano roll : un seul handler (capture) — désélection → aperçu → fermer.
    if (e.key === 'Escape' && hasNoModifiers(e) && !get(settingsPanelOpen)) {
      if (get(keyboardFocus) === 'library-results') {
        const lib = get(libraryStore);
        if (lib.query) {
          e.preventDefault();
          libraryStore.clearQuery();
          return;
        }
        if (lib.previewingId) {
          e.preventDefault();
          libraryStore.stopPreview();
          return;
        }
      }
      if (get(bottomPanel).open) {
        e.preventDefault();
        e.stopImmediatePropagation();
        handleEditorEscapeKey();
        return;
      }
    }

    for (const shortcut of SHORTCUTS) {
      if (!matches(e, shortcut)) continue;

      // Undo/redo are handled locally by the piano roll when it is focused.
      const isHistory =
        shortcut.key === 'z' || shortcut.key === 'y';
      if (isHistory && !shouldHandleGlobalHistoryShortcuts()) continue;

      e.preventDefault();
      shortcut.action();
      return; // first match wins
    }
  };

  window.addEventListener('keydown', handler, true);
  return () => window.removeEventListener('keydown', handler, true);
}
