/**
 * Canonical shortcut catalog — defaults and metadata.
 * Actions live in keymap.ts; this file stays free of side effects.
 */

import type { KeyBinding, ShortcutId, ShortcutMeta } from './types';

function chord(key: string, ...modifiers: KeyBinding['modifiers']): KeyBinding {
	return { key, modifiers: [...modifiers] };
}

export const SHORTCUT_CATALOG: readonly ShortcutMeta[] = [
	// ── File ──────────────────────────────────────────────────────────────────
	{
		id: 'file.new',
		category: 'file',
		labelKey: 'fileNew',
		defaultBinding: chord('n', 'ctrl'),
		scope: 'global',
		menuId: 'new'
	},
	{
		id: 'file.open',
		category: 'file',
		labelKey: 'fileOpen',
		defaultBinding: chord('o', 'ctrl'),
		scope: 'global',
		menuId: 'open'
	},
	{
		id: 'file.save',
		category: 'file',
		labelKey: 'fileSave',
		defaultBinding: chord('s', 'ctrl'),
		scope: 'global',
		menuId: 'save'
	},
	{
		id: 'file.saveAs',
		category: 'file',
		labelKey: 'fileSaveAs',
		defaultBinding: chord('s', 'ctrl', 'shift'),
		scope: 'global',
		menuId: 'save-as'
	},
	{
		id: 'file.importAudio',
		category: 'file',
		labelKey: 'fileImportAudio',
		defaultBinding: chord('i', 'ctrl', 'shift'),
		scope: 'global',
		menuId: 'import-audio'
	},
	{
		id: 'file.exportWav',
		category: 'file',
		labelKey: 'fileExportWav',
		defaultBinding: chord('e', 'ctrl', 'shift'),
		scope: 'global',
		menuId: 'export-wav'
	},
	{
		id: 'file.exportMp3',
		category: 'file',
		labelKey: 'fileExportMp3',
		defaultBinding: chord('m', 'ctrl', 'shift'),
		scope: 'global',
		menuId: 'export-mp3'
	},

	// ── Edit ──────────────────────────────────────────────────────────────────
	{
		id: 'edit.undo',
		category: 'edit',
		labelKey: 'editUndo',
		defaultBinding: chord('z', 'ctrl'),
		scope: 'global',
		menuId: 'undo',
		isHistory: true
	},
	{
		id: 'edit.redo',
		category: 'edit',
		labelKey: 'editRedo',
		defaultBinding: chord('z', 'ctrl', 'shift'),
		scope: 'global',
		menuId: 'redo',
		isHistory: true
	},
	{
		id: 'edit.redoAlt',
		category: 'edit',
		labelKey: 'editRedo',
		defaultBinding: chord('y', 'ctrl'),
		scope: 'global',
		aliasOf: 'edit.redo',
		isHistory: true
	},

	// ── Transport ─────────────────────────────────────────────────────────────
	{
		id: 'transport.playPause',
		category: 'transport',
		labelKey: 'transportPlayPause',
		defaultBinding: chord(' '),
		scope: 'global'
	},

	// ── Arrangement ───────────────────────────────────────────────────────────
	{
		id: 'arrangement.addMarker',
		category: 'arrangement',
		labelKey: 'arrangementAddMarker',
		defaultBinding: chord('m'),
		scope: 'global'
	},
	{
		id: 'arrangement.addRegion',
		category: 'arrangement',
		labelKey: 'arrangementAddRegion',
		defaultBinding: chord('r'),
		scope: 'global'
	},
	{
		id: 'arrangement.prevLocator',
		category: 'arrangement',
		labelKey: 'arrangementPrevLocator',
		defaultBinding: chord(','),
		scope: 'global'
	},
	{
		id: 'arrangement.nextLocator',
		category: 'arrangement',
		labelKey: 'arrangementNextLocator',
		defaultBinding: chord('.'),
		scope: 'global'
	},
	{
		id: 'arrangement.nextLocatorAlt',
		category: 'arrangement',
		labelKey: 'arrangementNextLocator',
		defaultBinding: chord(';'),
		scope: 'global',
		aliasOf: 'arrangement.nextLocator'
	},
	{
		id: 'arrangement.delete',
		category: 'arrangement',
		labelKey: 'arrangementDelete',
		defaultBinding: chord('Delete'),
		scope: 'timeline'
	},
	{
		id: 'arrangement.deleteAlt',
		category: 'arrangement',
		labelKey: 'arrangementDelete',
		defaultBinding: chord('Backspace'),
		scope: 'timeline',
		aliasOf: 'arrangement.delete'
	},
	{
		id: 'arrangement.copy',
		category: 'arrangement',
		labelKey: 'arrangementCopy',
		defaultBinding: chord('c', 'ctrl'),
		scope: 'timeline'
	},
	{
		id: 'arrangement.cut',
		category: 'arrangement',
		labelKey: 'arrangementCut',
		defaultBinding: chord('x', 'ctrl'),
		scope: 'timeline'
	},
	{
		id: 'arrangement.paste',
		category: 'arrangement',
		labelKey: 'arrangementPaste',
		defaultBinding: chord('v', 'ctrl'),
		scope: 'timeline'
	},
	{
		id: 'arrangement.duplicate',
		category: 'arrangement',
		labelKey: 'arrangementDuplicate',
		defaultBinding: chord('d', 'ctrl'),
		scope: 'timeline'
	},

	// ── Library ───────────────────────────────────────────────────────────────
	{
		id: 'library.toggle',
		category: 'library',
		labelKey: 'libraryToggle',
		defaultBinding: chord('b'),
		scope: 'global'
	},
	{
		id: 'library.toggleAlt',
		category: 'library',
		labelKey: 'libraryToggle',
		defaultBinding: chord('l'),
		scope: 'global',
		aliasOf: 'library.toggle'
	},
	{
		id: 'library.focusSearch',
		category: 'library',
		labelKey: 'libraryFocusSearch',
		defaultBinding: chord('/'),
		scope: 'global'
	},
	{
		id: 'library.focusSearchAlt',
		category: 'library',
		labelKey: 'libraryFocusSearch',
		defaultBinding: chord('f', 'ctrl'),
		scope: 'global',
		aliasOf: 'library.focusSearch'
	},
	{
		id: 'library.toggleFavorite',
		category: 'library',
		labelKey: 'libraryToggleFavorite',
		defaultBinding: chord('f'),
		scope: 'library'
	},
	{
		id: 'library.toggleFavoriteAlt',
		category: 'library',
		labelKey: 'libraryToggleFavorite',
		defaultBinding: chord('d', 'ctrl'),
		scope: 'library',
		aliasOf: 'library.toggleFavorite'
	},
	{
		id: 'library.sourceRecents',
		category: 'library',
		labelKey: 'librarySourceRecents',
		defaultBinding: chord('1', 'ctrl'),
		scope: 'library'
	},
	{
		id: 'library.sourceFavorites',
		category: 'library',
		labelKey: 'librarySourceFavorites',
		defaultBinding: chord('2', 'ctrl'),
		scope: 'library'
	},
	{
		id: 'library.sourceAudio',
		category: 'library',
		labelKey: 'librarySourceAudio',
		defaultBinding: chord('3', 'ctrl'),
		scope: 'library'
	},

	// ── View / editor ─────────────────────────────────────────────────────────
	{
		id: 'view.toggleMain',
		category: 'view',
		labelKey: 'viewToggleMain',
		defaultBinding: chord('g'),
		scope: 'global'
	},
	{
		id: 'editor.cycleFocus',
		category: 'editor',
		labelKey: 'editorCycleFocus',
		defaultBinding: chord('Tab'),
		scope: 'global'
	},
	{
		id: 'track.add',
		category: 'track',
		labelKey: 'trackAdd',
		defaultBinding: chord('t', 'ctrl', 'shift'),
		scope: 'global',
		menuId: 'add-track'
	},

	// ── Piano roll ────────────────────────────────────────────────────────────
	{
		id: 'pianoRoll.modeDraw',
		category: 'pianoRoll',
		labelKey: 'pianoRollModeDraw',
		defaultBinding: chord('d'),
		scope: 'piano-roll'
	},
	{
		id: 'pianoRoll.modeSelect',
		category: 'pianoRoll',
		labelKey: 'pianoRollModeSelect',
		defaultBinding: chord('v'),
		scope: 'piano-roll'
	},
	{
		id: 'pianoRoll.modeSelectAlt',
		category: 'pianoRoll',
		labelKey: 'pianoRollModeSelect',
		defaultBinding: chord('s'),
		scope: 'piano-roll',
		aliasOf: 'pianoRoll.modeSelect'
	},
	{
		id: 'pianoRoll.modeErase',
		category: 'pianoRoll',
		labelKey: 'pianoRollModeErase',
		defaultBinding: chord('e'),
		scope: 'piano-roll'
	},
	{
		id: 'pianoRoll.quantize',
		category: 'pianoRoll',
		labelKey: 'pianoRollQuantize',
		defaultBinding: chord('q'),
		scope: 'piano-roll'
	},
	{
		id: 'pianoRoll.quantizeAlt',
		category: 'pianoRoll',
		labelKey: 'pianoRollQuantize',
		defaultBinding: chord('u', 'ctrl'),
		scope: 'piano-roll',
		aliasOf: 'pianoRoll.quantize'
	},
	{
		id: 'pianoRoll.delete',
		category: 'pianoRoll',
		labelKey: 'pianoRollDelete',
		defaultBinding: chord('Delete'),
		scope: 'piano-roll'
	},
	{
		id: 'pianoRoll.deleteAlt',
		category: 'pianoRoll',
		labelKey: 'pianoRollDelete',
		defaultBinding: chord('Backspace'),
		scope: 'piano-roll',
		aliasOf: 'pianoRoll.delete'
	},
	{
		id: 'pianoRoll.copy',
		category: 'pianoRoll',
		labelKey: 'pianoRollCopy',
		defaultBinding: chord('c', 'ctrl'),
		scope: 'piano-roll'
	},
	{
		id: 'pianoRoll.cut',
		category: 'pianoRoll',
		labelKey: 'pianoRollCut',
		defaultBinding: chord('x', 'ctrl'),
		scope: 'piano-roll'
	},
	{
		id: 'pianoRoll.paste',
		category: 'pianoRoll',
		labelKey: 'pianoRollPaste',
		defaultBinding: chord('v', 'ctrl'),
		scope: 'piano-roll'
	},
	{
		id: 'pianoRoll.duplicate',
		category: 'pianoRoll',
		labelKey: 'pianoRollDuplicate',
		defaultBinding: chord('d', 'ctrl'),
		scope: 'piano-roll'
	},
	{
		id: 'pianoRoll.selectAll',
		category: 'pianoRoll',
		labelKey: 'pianoRollSelectAll',
		defaultBinding: chord('a', 'ctrl'),
		scope: 'piano-roll'
	}
] as const;

const BY_ID = new Map(SHORTCUT_CATALOG.map((m) => [m.id, m]));

export function getShortcutMeta(id: ShortcutId): ShortcutMeta {
	const meta = BY_ID.get(id);
	if (!meta) throw new Error(`Unknown shortcut id: ${id}`);
	return meta;
}

/** Primary action id (unwraps `aliasOf`). */
export function rootShortcutId(id: ShortcutId): ShortcutId {
	return getShortcutMeta(id).aliasOf ?? id;
}

/** Alternate chords that belong under a primary Settings row. */
export function aliasesOf(id: ShortcutId): readonly ShortcutMeta[] {
	return SHORTCUT_CATALOG.filter((m) => m.aliasOf === id);
}

/** Catalog entries shown as Settings rows (aliases folded into their primary). */
export function settingsCatalog(): readonly ShortcutMeta[] {
	return SHORTCUT_CATALOG.filter((m) => !m.aliasOf);
}

/** Primary + its alias entries, in display order. */
export function chordsForRow(id: ShortcutId): readonly ShortcutMeta[] {
	return [getShortcutMeta(id), ...aliasesOf(id)];
}

export const SHORTCUT_CATEGORY_ORDER: ShortcutMeta['category'][] = [
	'file',
	'edit',
	'transport',
	'arrangement',
	'track',
	'library',
	'view',
	'editor',
	'pianoRoll'
];
