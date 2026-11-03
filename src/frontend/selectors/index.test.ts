import { describe, expect, it } from 'vitest';
import { midiContentOf } from './index';

describe('midiContentOf', () => {
	const content = { notes: [], channel_controls: [] };

	it('returns the content the clip references', () => {
		expect(midiContentOf({ midi_contents: { c1: content } }, { content_id: 'c1' })).toBe(content);
	});

	it('returns the same empty content when the pool has no entry', () => {
		const state = { midi_contents: {} };
		const first = midiContentOf(state, { content_id: 'missing' });
		expect(first).toEqual({ notes: [], channel_controls: [] });
		expect(midiContentOf(state, { content_id: 'other' })).toBe(first);
	});
});
