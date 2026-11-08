/**
 * Keyboard shortcut manager — single registration point for the app.
 *
 * Architecture:
 * - Registered once in the root +page.svelte via onMount → cleanup on destroy.
 * - File operations (new/open/save) go through the `menu_action` Tauri command,
 *   which reuses the exact same Rust logic as the native menu items.
 * - History operations (undo/redo) call the existing dispatcher helpers.
 * - Bindings come from the catalog + user overrides in preferences.
 * - Shortcuts are intentionally suppressed when the user is typing in an input.
 */

import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import {
	undoCommand,
	redoCommand,
	play,
	pause,
	addMarker,
	addRegion,
	seek
} from '../commands/dispatcher';
import { transport } from '../../stores/transport';
import { tempo } from '../../stores/tempo';
import { loopRegion } from '../../stores/loopRegion';
import { notify } from '../../stores/status';
import { isEditingText, shouldHandleGlobalHistoryShortcuts, hasNoModifiers } from './context';
import { engineProject } from '../store/engineStore';
import { selection } from '../../stores/selection';
import { bottomPanel } from '../../stores/bottomPanel';
import { preferences, settingsPanelOpen } from '../../stores/preferences';
import { cycleEditorFocus, handleEditorEscapeKey } from '../pianoRoll/clipEditor';
import { viewMode } from '../../stores/viewMode';
import { dismissTopLayer } from '../dismiss';
import { rightPanel } from '../../stores/rightPanel';
import { keyboardFocus } from '../../stores/keyboardFocus';
import { libraryStore, libraryPopup, requestLibrarySearchFocus, beatsToSeconds } from '../library';
import { collectLocators, navigateLocator, defaultRegionSpanAt } from '../arrangement/locators';
import { SHORTCUT_CATALOG } from './catalog';
import { matchesShortcut, menuAccelerators } from './bindings';
import { shortcutRecording } from './recording';
import type { ShortcutId, ShortcutOverrides } from './types';

function isLibraryFocus(): boolean {
	const focus = get(keyboardFocus);
	return focus === 'library-results' || focus === 'library-search';
}

function playheadBeat(): number {
	const bpm = get(tempo).bpm;
	return (get(transport).position * bpm) / 60;
}

/** Jump to previous/next locator relative to the playhead (no selection state). */
function navigateLocators(direction: -1 | 1) {
	const project = get(engineProject);
	const entries = collectLocators(project.markers, project.regions);
	const next = navigateLocator(entries, null, playheadBeat(), direction);
	if (!next) return;
	selection.clear();
	keyboardFocus.setTimeline();
	const positionSec = beatsToSeconds(Math.max(0, next.beat), get(tempo).bpm);
	transport.update((t) => ({ ...t, position: positionSec }));
	seek(positionSec).catch(console.error);
}

const menuAction = (action: string) => () => invoke('menu_action', { action }).catch(console.error);

type ActionMap = Record<ShortcutId, () => void>;

function buildActions(): ActionMap {
	return {
		'file.new': menuAction('new'),
		'file.open': menuAction('open'),
		'file.save': menuAction('save'),
		'file.saveAs': menuAction('save-as'),
		'file.importAudio': menuAction('import-audio'),
		'file.exportWav': menuAction('export-wav'),
		'file.exportMp3': menuAction('export-mp3'),
		'edit.undo': () => undoCommand().catch((e) => notify(`Annuler : ${e?.message ?? e}`, 'error')),
		'edit.redo': () => redoCommand().catch((e) => notify(`Rétablir : ${e?.message ?? e}`, 'error')),
		'edit.redoAlt': () =>
			redoCommand().catch((e) => notify(`Rétablir : ${e?.message ?? e}`, 'error')),
		'transport.playPause': () => {
			if (get(keyboardFocus) === 'library-results') {
				const selected = libraryStore.getSelectedAsset();
				if (selected) {
					void libraryStore
						.togglePreview(selected.id)
						.catch((e) => notify(`Aperçu : ${e?.message ?? e}`, 'error'));
					return;
				}
			}
			const { isPlaying } = get(transport);
			(isPlaying ? pause() : play()).catch(console.error);
		},
		'arrangement.addMarker': () => {
			const bpm = get(tempo).bpm;
			const beat = (get(transport).position * bpm) / 60;
			addMarker(Math.max(0, beat)).catch(console.error);
		},
		'arrangement.addRegion': () => {
			const loop = get(loopRegion);
			if (loop.enabled) {
				addRegion(loop.startBeat, loop.endBeat).catch(console.error);
				return;
			}
			const bpm = get(tempo).bpm;
			const numerator = get(tempo).numerator;
			const beat = (get(transport).position * bpm) / 60;
			const { start, end } = defaultRegionSpanAt(beat, numerator);
			addRegion(start, end).catch(console.error);
		},
		'arrangement.prevLocator': () => navigateLocators(-1),
		'arrangement.nextLocator': () => navigateLocators(1),
		'arrangement.nextLocatorAlt': () => navigateLocators(1),
		'arrangement.delete': () => {},
		'arrangement.deleteAlt': () => {},
		'arrangement.copy': () => {},
		'arrangement.cut': () => {},
		'arrangement.paste': () => {},
		'arrangement.duplicate': () => {},
		'library.toggle': () => rightPanel.toggle(),
		'library.toggleAlt': () => rightPanel.toggle(),
		'library.focusSearch': () => {
			if (get(libraryPopup).open) {
				requestLibrarySearchFocus();
				return;
			}
			if (!get(rightPanel).open) rightPanel.toggle();
			requestLibrarySearchFocus();
		},
		'library.focusSearchAlt': () => {
			if (!get(rightPanel).open) return;
			requestLibrarySearchFocus();
		},
		'library.toggleFavorite': () => {
			const selected = libraryStore.getSelectedAsset();
			if (selected) libraryStore.toggleFavorite(selected.id);
		},
		'library.toggleFavoriteAlt': () => {
			const selected = libraryStore.getSelectedAsset();
			if (selected) libraryStore.toggleFavorite(selected.id);
		},
		'library.sourceRecents': () => libraryStore.setSource('recents'),
		'library.sourceFavorites': () => libraryStore.setSource('favorites'),
		'library.sourceAudio': () => libraryStore.setSource('audio'),
		'view.toggleMain': () => viewMode.toggleMain(),
		'editor.cycleFocus': () => cycleEditorFocus(get(engineProject), get(selection)),
		'track.add': menuAction('add-track'),
		'pianoRoll.modeDraw': () => {},
		'pianoRoll.modeSelect': () => {},
		'pianoRoll.modeSelectAlt': () => {},
		'pianoRoll.modeErase': () => {},
		'pianoRoll.quantize': () => {},
		'pianoRoll.quantizeAlt': () => {},
		'pianoRoll.delete': () => {},
		'pianoRoll.deleteAlt': () => {},
		'pianoRoll.copy': () => {},
		'pianoRoll.cut': () => {},
		'pianoRoll.paste': () => {},
		'pianoRoll.duplicate': () => {},
		'pianoRoll.selectAll': () => {}
	};
}

function scopeAllows(id: ShortcutId): boolean {
	const meta = SHORTCUT_CATALOG.find((m) => m.id === id);
	if (!meta) return false;
	if (meta.scope === 'library') return isLibraryFocus();
	if (meta.scope === 'timeline' || meta.scope === 'piano-roll') return false;
	return true;
}

function currentOverrides(): ShortcutOverrides {
	return get(preferences).shortcuts;
}

/** Push effective menu accelerators to the native OS menu. */
export function syncMenuAccelerators(overrides: ShortcutOverrides = currentOverrides()): void {
	const accelerators = menuAccelerators(overrides);
	invoke('set_menu_accelerators', { accelerators }).catch(() => {
		// Menu may not be ready yet during boot — ignore.
	});
}

/**
 * Attach the global keyboard handler to `window`.
 *
 * @returns A cleanup function — pass it directly as the onMount return value.
 */
export function registerShortcuts(): () => void {
	const actions = buildActions();

	const handler = (e: KeyboardEvent): void => {
		if (e.repeat) return;
		if (get(shortcutRecording)) return;

		if (e.key === 'Escape' && dismissTopLayer()) {
			e.preventDefault();
			e.stopImmediatePropagation();
			return;
		}

		const overrides = currentOverrides();

		if (isEditingText(e.target)) {
			// ADR-023: library toggle still works from the search field.
			if (
				matchesShortcut(e, 'library.toggle', overrides) ||
				matchesShortcut(e, 'library.toggleAlt', overrides)
			) {
				e.preventDefault();
				rightPanel.toggle();
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

		for (const meta of SHORTCUT_CATALOG) {
			if (meta.scope === 'timeline' || meta.scope === 'piano-roll') continue;
			if (!matchesShortcut(e, meta.id, overrides)) continue;
			if (!scopeAllows(meta.id)) continue;

			if (meta.isHistory && !shouldHandleGlobalHistoryShortcuts()) continue;

			e.preventDefault();
			actions[meta.id]();
			return;
		}
	};

	window.addEventListener('keydown', handler, true);
	syncMenuAccelerators();
	return () => window.removeEventListener('keydown', handler, true);
}
