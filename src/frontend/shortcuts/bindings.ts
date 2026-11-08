/**
 * Binding helpers — normalize, match, format, conflict detection, menu sync.
 */

import {
	aliasesOf,
	chordsForRow,
	getShortcutMeta,
	rootShortcutId,
	SHORTCUT_CATALOG
} from './catalog';
import type { KeyBinding, Modifier, ShortcutId, ShortcutOverrides, ShortcutScope } from './types';

const MOD_ORDER: Modifier[] = ['ctrl', 'shift', 'alt'];

/** Stable key + sorted unique modifiers. */
export function normalizeBinding(binding: KeyBinding): KeyBinding {
	const seen = new Set<Modifier>();
	const modifiers: Modifier[] = [];
	for (const m of MOD_ORDER) {
		if (binding.modifiers.includes(m) && !seen.has(m)) {
			seen.add(m);
			modifiers.push(m);
		}
	}
	return { key: binding.key.length === 1 ? binding.key.toLowerCase() : binding.key, modifiers };
}

export function bindingsEqual(a: KeyBinding, b: KeyBinding): boolean {
	const na = normalizeBinding(a);
	const nb = normalizeBinding(b);
	if (na.key.toLowerCase() !== nb.key.toLowerCase()) return false;
	if (na.modifiers.length !== nb.modifiers.length) return false;
	return na.modifiers.every((m, i) => m === nb.modifiers[i]);
}

export function resolveBinding(id: ShortcutId, overrides: ShortcutOverrides = {}): KeyBinding {
	const override = overrides[id];
	if (override) return normalizeBinding(override);
	return normalizeBinding(getShortcutMeta(id).defaultBinding);
}

/** Primary + aliases (aliases only while the user has not overridden). */
export function resolveMatchBindings(
	id: ShortcutId,
	overrides: ShortcutOverrides = {}
): KeyBinding[] {
	const meta = getShortcutMeta(id);
	if (overrides[id]) return [normalizeBinding(overrides[id]!)];
	const list = [meta.defaultBinding, ...(meta.defaultAliases ?? [])];
	return list.map(normalizeBinding);
}

export function isCustomized(id: ShortcutId, overrides: ShortcutOverrides): boolean {
	const override = overrides[id];
	if (!override) return false;
	return !bindingsEqual(override, getShortcutMeta(id).defaultBinding);
}

/**
 * True when the event exactly satisfies a binding's key + modifiers.
 * `ctrl` maps to Cmd on macOS (metaKey) and Ctrl on Windows/Linux.
 */
export function matchesBinding(e: KeyboardEvent, binding: KeyBinding): boolean {
	const needs = normalizeBinding(binding);
	const eventKey = e.key.length === 1 ? e.key.toLowerCase() : e.key;
	if (eventKey.toLowerCase() !== needs.key.toLowerCase()) return false;

	const ctrlHeld = e.ctrlKey || e.metaKey;
	if (needs.modifiers.includes('ctrl') !== ctrlHeld) return false;
	if (needs.modifiers.includes('shift') !== e.shiftKey) return false;
	if (needs.modifiers.includes('alt') !== e.altKey) return false;
	return true;
}

export function matchesShortcut(
	e: KeyboardEvent,
	id: ShortcutId,
	overrides: ShortcutOverrides = {}
): boolean {
	return resolveMatchBindings(id, overrides).some((b) => matchesBinding(e, b));
}

/** Match a Settings row: primary chord or any of its `aliasOf` chips. */
export function matchesShortcutRow(
	e: KeyboardEvent,
	rowId: ShortcutId,
	overrides: ShortcutOverrides = {}
): boolean {
	return chordsForRow(rowId).some((m) => matchesShortcut(e, m.id, overrides));
}

/** Scopes that may share a chord without counting as a conflict. */
function scopesConflict(a: ShortcutScope, b: ShortcutScope): boolean {
	if (a === b) return true;
	const focusPair = (x: ShortcutScope, y: ShortcutScope) =>
		(a === x && b === y) || (a === y && b === x);
	// Focus-scoped layers never run at the same time.
	if (focusPair('library', 'timeline')) return false;
	if (focusPair('library', 'piano-roll')) return false;
	if (focusPair('timeline', 'piano-roll')) return false;
	if (a === 'global' || b === 'global') return true;
	return true;
}

export interface ShortcutConflict {
	id: ShortcutId;
	otherId: ShortcutId;
	binding: KeyBinding;
}

/** Conflicts among effective bindings (same-action aliases ignored). */
export function findConflicts(overrides: ShortcutOverrides = {}): ShortcutConflict[] {
	const conflicts: ShortcutConflict[] = [];
	const entries = SHORTCUT_CATALOG.map((m) => ({
		id: m.id,
		scope: m.scope,
		binding: resolveBinding(m.id, overrides)
	}));

	for (let i = 0; i < entries.length; i++) {
		for (let j = i + 1; j < entries.length; j++) {
			const a = entries[i];
			const b = entries[j];
			if (rootShortcutId(a.id) === rootShortcutId(b.id)) continue;
			if (!scopesConflict(a.scope, b.scope)) continue;
			if (!bindingsEqual(a.binding, b.binding)) continue;
			conflicts.push({ id: a.id, otherId: b.id, binding: a.binding });
		}
	}
	return conflicts;
}

export function conflictIdsFor(id: ShortcutId, overrides: ShortcutOverrides = {}): ShortcutId[] {
	return findConflicts(overrides)
		.filter((c) => c.id === id || c.otherId === id)
		.map((c) => (c.id === id ? c.otherId : c.id));
}

/**
 * Conflicting primary actions for a Settings row (primary + its alias chips).
 * Returns root ids suitable for display labels.
 */
export function rowConflictRoots(
	rowId: ShortcutId,
	overrides: ShortcutOverrides = {}
): ShortcutId[] {
	const rowIds = new Set<ShortcutId>([rowId, ...aliasesOf(rowId).map((a) => a.id)]);
	const roots = new Set<ShortcutId>();
	for (const sid of rowIds) {
		for (const other of conflictIdsFor(sid, overrides)) {
			const root = rootShortcutId(other);
			if (!rowIds.has(other) && root !== rowId) roots.add(root);
		}
	}
	return [...roots];
}

export interface FormatBindingOptions {
	apple?: boolean;
	/** `fr` → Suppr for Delete; otherwise Del. */
	locale?: string | null;
}

function displayKey(raw: string, locale?: string | null): string {
	if (raw === 'Delete' || raw.toLowerCase() === 'delete') {
		return locale?.toLowerCase().startsWith('fr') ? 'Suppr' : 'Del';
	}
	const map: Record<string, string> = {
		' ': 'Space',
		ArrowUp: '↑',
		ArrowDown: '↓',
		ArrowLeft: '←',
		ArrowRight: '→',
		Backspace: '⌫',
		Escape: 'Esc',
		Tab: 'Tab',
		Enter: 'Enter'
	};
	return map[raw] ?? map[raw.toLowerCase()] ?? (raw.length === 1 ? raw.toUpperCase() : raw);
}

export function isApplePlatform(): boolean {
	if (typeof navigator === 'undefined') return false;
	return /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent);
}

/** Human-readable chord for menus / settings (Ctrl vs ⌘). */
export function formatBinding(
	binding: KeyBinding,
	appleOrOpts: boolean | FormatBindingOptions = isApplePlatform()
): string {
	const opts: FormatBindingOptions =
		typeof appleOrOpts === 'boolean' ? { apple: appleOrOpts } : appleOrOpts;
	const apple = opts.apple ?? isApplePlatform();
	const n = normalizeBinding(binding);
	const parts: string[] = [];
	if (n.modifiers.includes('ctrl')) parts.push(apple ? '⌘' : 'Ctrl');
	if (n.modifiers.includes('shift')) parts.push(apple ? '⇧' : 'Shift');
	if (n.modifiers.includes('alt')) parts.push(apple ? '⌥' : 'Alt');
	parts.push(displayKey(n.key, opts.locale));
	return parts.join(apple ? '' : '+');
}

const FORBIDDEN_SOLO_KEYS = new Set(['Escape']);

export type BindingCaptureResult = KeyBinding | 'cancel' | 'incomplete' | 'forbidden';

/**
 * Build a binding from a KeyboardEvent during capture.
 * - `incomplete`: pure modifier key (wait for the rest)
 * - `forbidden`: reserved solo key
 * - `cancel`: Escape
 */
export function bindingFromEvent(e: KeyboardEvent): BindingCaptureResult {
	if (e.key === 'Escape') return 'cancel';
	if (e.key === 'Control' || e.key === 'Shift' || e.key === 'Alt' || e.key === 'Meta') {
		return 'incomplete';
	}
	if (FORBIDDEN_SOLO_KEYS.has(e.key) && !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
		return 'forbidden';
	}

	const modifiers: Modifier[] = [];
	if (e.ctrlKey || e.metaKey) modifiers.push('ctrl');
	if (e.shiftKey) modifiers.push('shift');
	if (e.altKey) modifiers.push('alt');

	const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
	return normalizeBinding({ key, modifiers });
}

const TAURI_KEY: Record<string, string> = {
	' ': 'Space',
	',': 'Comma',
	'.': 'Period',
	';': 'Semicolon',
	'/': 'Slash',
	Tab: 'Tab',
	Delete: 'Delete',
	Backspace: 'Backspace',
	Enter: 'Enter',
	ArrowUp: 'Up',
	ArrowDown: 'Down',
	ArrowLeft: 'Left',
	ArrowRight: 'Right'
};

/** muda / Tauri accelerator string (`CmdOrCtrl+Shift+S`). */
export function toTauriAccelerator(binding: KeyBinding): string {
	const n = normalizeBinding(binding);
	const parts: string[] = [];
	if (n.modifiers.includes('ctrl')) parts.push('CmdOrCtrl');
	if (n.modifiers.includes('shift')) parts.push('Shift');
	if (n.modifiers.includes('alt')) parts.push('Alt');
	const key =
		TAURI_KEY[n.key] ??
		TAURI_KEY[n.key.toLowerCase()] ??
		(n.key.length === 1 ? n.key.toUpperCase() : n.key);
	parts.push(key);
	return parts.join('+');
}

/** Map of native menu id → accelerator for every catalog entry that has a menuId. */
export function menuAccelerators(overrides: ShortcutOverrides = {}): Record<string, string> {
	const out: Record<string, string> = {};
	for (const meta of SHORTCUT_CATALOG) {
		if (!meta.menuId) continue;
		out[meta.menuId] = toTauriAccelerator(resolveBinding(meta.id, overrides));
	}
	return out;
}
