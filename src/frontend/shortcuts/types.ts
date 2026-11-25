/**
 * Shared types for the configurable keyboard shortcut system.
 */

export type Modifier = 'ctrl' | 'shift' | 'alt';

/** Normalized key chord — `ctrl` means Cmd on macOS, Ctrl elsewhere. */
export interface KeyBinding {
	key: string;
	modifiers: Modifier[];
}

export type ShortcutCategory =
	| 'file'
	| 'edit'
	| 'transport'
	| 'arrangement'
	| 'library'
	| 'view'
	| 'editor'
	| 'track'
	| 'pianoRoll';

/** Where the shortcut is eligible to fire. */
export type ShortcutScope = 'global' | 'library' | 'timeline' | 'piano-roll';

export type ShortcutId =
	| 'file.new'
	| 'file.open'
	| 'file.save'
	| 'file.saveAs'
	| 'file.importAudio'
	| 'file.exportWav'
	| 'file.exportMp3'
	| 'edit.undo'
	| 'edit.redo'
	| 'edit.redoAlt'
	| 'transport.playPause'
	| 'transport.toggleLoop'
	| 'arrangement.addMarker'
	| 'arrangement.addRegion'
	| 'arrangement.prevLocator'
	| 'arrangement.nextLocator'
	| 'arrangement.nextLocatorAlt'
	| 'arrangement.delete'
	| 'arrangement.deleteAlt'
	| 'arrangement.copy'
	| 'arrangement.cut'
	| 'arrangement.paste'
	| 'arrangement.duplicate'
	| 'library.toggle'
	| 'library.focusSearch'
	| 'library.focusSearchAlt'
	| 'library.toggleFavorite'
	| 'library.toggleFavoriteAlt'
	| 'library.sourceRecents'
	| 'library.sourceFavorites'
	| 'library.sourceAudio'
	| 'view.toggleMain'
	| 'editor.cycleFocus'
	| 'track.add'
	| 'pianoRoll.modeDraw'
	| 'pianoRoll.modeSelect'
	| 'pianoRoll.modeSelectAlt'
	| 'pianoRoll.modeErase'
	| 'pianoRoll.quantize'
	| 'pianoRoll.quantizeAlt'
	| 'pianoRoll.delete'
	| 'pianoRoll.deleteAlt'
	| 'pianoRoll.copy'
	| 'pianoRoll.cut'
	| 'pianoRoll.paste'
	| 'pianoRoll.duplicate'
	| 'pianoRoll.selectAll';

/** Overrides keyed by shortcut id — absent id → use catalog default. */
export type ShortcutOverrides = Partial<Record<ShortcutId, KeyBinding>>;

export interface ShortcutMeta {
	id: ShortcutId;
	category: ShortcutCategory;
	/** svelte-i18n key under `shortcuts.actions.*`. */
	labelKey: string;
	defaultBinding: KeyBinding;
	scope: ShortcutScope;
	/** Extra chords that match until the user overrides this shortcut. */
	defaultAliases?: KeyBinding[];
	/**
	 * Alternate chord for another action — hidden as its own Settings row;
	 * shown as an extra chip on the primary action instead.
	 */
	aliasOf?: ShortcutId;
	/** Native menu item id (`MenuItem::with_id`) when this drives an OS accelerator. */
	menuId?: string;
	/** Undo/redo — suppressed while the piano roll owns history. */
	isHistory?: boolean;
}
