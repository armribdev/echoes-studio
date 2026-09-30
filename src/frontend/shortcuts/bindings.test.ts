import { describe, expect, it } from 'vitest';
import {
	bindingsEqual,
	conflictIdsFor,
	findConflicts,
	formatBinding,
	matchesBinding,
	normalizeBinding,
	resolveBinding,
	resolveMatchBindings,
	rowConflictRoots,
	toTauriAccelerator
} from './bindings';
import { aliasesOf, settingsCatalog } from './catalog';

function fakeKey(partial: Partial<KeyboardEvent> & { key: string }): KeyboardEvent {
	return {
		key: partial.key,
		ctrlKey: partial.ctrlKey ?? false,
		metaKey: partial.metaKey ?? false,
		shiftKey: partial.shiftKey ?? false,
		altKey: partial.altKey ?? false
	} as KeyboardEvent;
}

describe('normalizeBinding / bindingsEqual', () => {
	it('lowercases single-character keys and sorts modifiers', () => {
		expect(normalizeBinding({ key: 'S', modifiers: ['shift', 'ctrl'] })).toEqual({
			key: 's',
			modifiers: ['ctrl', 'shift']
		});
	});

	it('treats equivalent bindings as equal', () => {
		expect(
			bindingsEqual(
				{ key: 'S', modifiers: ['shift', 'ctrl'] },
				{ key: 's', modifiers: ['ctrl', 'shift'] }
			)
		).toBe(true);
	});
});

describe('resolveBinding', () => {
	it('returns the catalog default when no override', () => {
		expect(resolveBinding('file.save')).toEqual({ key: 's', modifiers: ['ctrl'] });
	});

	it('returns the override when present', () => {
		expect(resolveBinding('file.save', { 'file.save': { key: 'p', modifiers: ['ctrl'] } })).toEqual(
			{ key: 'p', modifiers: ['ctrl'] }
		);
	});

	it('keeps Delete/Backspace as separate chips under the same row', () => {
		expect(resolveMatchBindings('arrangement.delete').map((b) => b.key)).toEqual(['Delete']);
		expect(resolveMatchBindings('arrangement.deleteAlt').map((b) => b.key)).toEqual(['Backspace']);
		expect(aliasesOf('arrangement.delete').map((a) => a.id)).toEqual(['arrangement.deleteAlt']);
	});
});

describe('matchesBinding', () => {
	it('matches ctrl/meta as the ctrl modifier', () => {
		const binding: { key: string; modifiers: ('ctrl' | 'shift' | 'alt')[] } = {
			key: 's',
			modifiers: ['ctrl']
		};
		expect(matchesBinding(fakeKey({ key: 's', ctrlKey: true }), binding)).toBe(true);
		expect(matchesBinding(fakeKey({ key: 's', metaKey: true }), binding)).toBe(true);
		expect(matchesBinding(fakeKey({ key: 's', shiftKey: true }), binding)).toBe(false);
	});
});

describe('settings rows / aliases', () => {
	it('folds alias entries out of the settings catalog', () => {
		const ids = settingsCatalog().map((m) => m.id);
		expect(ids).toContain('library.toggle');
		expect(ids).not.toContain('library.toggleAlt');
		expect(aliasesOf('library.toggle').map((a) => a.id)).toEqual(['library.toggleAlt']);
	});
});

describe('findConflicts', () => {
	it('reports no conflicts for defaults', () => {
		expect(findConflicts({})).toEqual([]);
	});

	it('detects a global conflict after remapping', () => {
		const overrides = {
			'library.toggle': { key: 'g', modifiers: [] as ('ctrl' | 'shift' | 'alt')[] }
		};
		const conflicts = findConflicts(overrides);
		expect(conflicts.some((c) => c.id === 'library.toggle' || c.otherId === 'library.toggle')).toBe(
			true
		);
		expect(conflictIdsFor('library.toggle', overrides)).toContain('view.toggleMain');
		expect(rowConflictRoots('library.toggle', overrides)).toContain('view.toggleMain');
	});

	it('does not treat primary vs its own alias as a conflict', () => {
		const overrides = {
			'library.toggleAlt': { key: 'b', modifiers: [] as ('ctrl' | 'shift' | 'alt')[] }
		};
		expect(conflictIdsFor('library.toggle', overrides)).not.toContain('library.toggleAlt');
		expect(rowConflictRoots('library.toggle', overrides)).toEqual([]);
	});

	it('allows piano-roll and timeline to share Delete', () => {
		expect(conflictIdsFor('pianoRoll.delete', {})).not.toContain('arrangement.delete');
		expect(rowConflictRoots('pianoRoll.delete', {})).toEqual([]);
	});

	it('allows library and timeline scopes to share a chord', () => {
		const overrides = {
			'library.toggleFavorite': { key: 'c', modifiers: ['ctrl'] as ('ctrl' | 'shift' | 'alt')[] }
		};
		expect(conflictIdsFor('library.toggleFavorite', overrides)).not.toContain('arrangement.copy');
	});
});

describe('formatBinding / toTauriAccelerator', () => {
	it('formats Windows-style chords', () => {
		expect(formatBinding({ key: 's', modifiers: ['ctrl', 'shift'] }, false)).toBe('Ctrl+Shift+S');
		expect(formatBinding({ key: ' ', modifiers: [] }, false)).toBe('Space');
	});

	it('localizes Delete', () => {
		expect(formatBinding({ key: 'Delete', modifiers: [] }, { apple: false, locale: 'fr' })).toBe(
			'Suppr'
		);
		expect(formatBinding({ key: 'Delete', modifiers: [] }, { apple: false, locale: 'en' })).toBe(
			'Del'
		);
	});

	it('builds muda accelerator strings', () => {
		expect(toTauriAccelerator({ key: 's', modifiers: ['ctrl'] })).toBe('CmdOrCtrl+S');
		expect(toTauriAccelerator({ key: 's', modifiers: ['ctrl', 'shift'] })).toBe(
			'CmdOrCtrl+Shift+S'
		);
		expect(toTauriAccelerator({ key: ',', modifiers: [] })).toBe('Comma');
	});
});
