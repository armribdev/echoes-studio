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
    action: () => undoCommand().catch(console.error),
  },
  {
    key: 'z',
    modifiers: ['ctrl', 'shift'],
    description: 'Rétablir',
    action: () => redoCommand().catch(console.error),
  },
  {
    key: 'y',
    modifiers: ['ctrl'],
    description: 'Rétablir (Windows)',
    action: () => redoCommand().catch(console.error),
  },

  // ── Transport ─────────────────────────────────────────────────────────────
  {
    key: ' ',
    modifiers: [],
    description: 'Lecture / Pause',
    action: () => {
      const { isPlaying } = get(transport);
      (isPlaying ? pause() : play()).catch(console.error);
    },
  },
];

// ── Matching logic ────────────────────────────────────────────────────────────

/** Returns true when the event target is an editable element. */
function isEditingText(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  const tag = target.tagName.toLowerCase();
  return (
    tag === 'input' ||
    tag === 'textarea' ||
    tag === 'select' ||
    (target as HTMLElement).isContentEditable
  );
}

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
    if (e.repeat) return; // ignore key-repeat to avoid spawning multiple Rust threads
    if (isEditingText(e.target)) return;

    for (const shortcut of SHORTCUTS) {
      if (matches(e, shortcut)) {
        e.preventDefault();
        shortcut.action();
        return; // first match wins
      }
    }
  };

  window.addEventListener('keydown', handler);
  return () => window.removeEventListener('keydown', handler);
}
