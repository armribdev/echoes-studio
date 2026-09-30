import { get } from 'svelte/store';
import { beforeEach, describe, expect, it } from 'vitest';
import { selection } from './selection';

beforeEach(() => {
	selection.clear();
});

describe('selection store', () => {
	it('selects a single clip as primary', () => {
		selection.select('a');
		expect(get(selection)).toEqual({ ids: new Set(['a']), primaryId: 'a' });
	});

	it('toggles additive multi-select', () => {
		selection.select('a');
		selection.select('b', { additive: true });
		expect(get(selection).ids).toEqual(new Set(['a', 'b']));
		expect(get(selection).primaryId).toBe('b');

		selection.select('a', { additive: true });
		expect(get(selection).ids).toEqual(new Set(['b']));
		expect(get(selection).primaryId).toBe('a');
	});

	it('setSingle and setMany replace the selection', () => {
		selection.select('a');
		selection.setSingle('b');
		expect(get(selection)).toEqual({ ids: new Set(['b']), primaryId: 'b' });

		selection.setMany(['c', 'd']);
		expect(get(selection).ids).toEqual(new Set(['c', 'd']));
		expect(get(selection).primaryId).toBe('d');
	});

	it('remaps pending ids after engine confirmation', () => {
		selection.setMany(['keep', 'pending']);
		selection.remapId('pending', 'real');
		expect(get(selection).ids).toEqual(new Set(['keep', 'real']));
		expect(get(selection).primaryId).toBe('real');
	});

	it('clears to empty state', () => {
		selection.setSingle('a');
		selection.clear();
		expect(get(selection)).toEqual({ ids: new Set(), primaryId: null });
	});
});
