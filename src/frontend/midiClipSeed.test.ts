import { describe, expect, it } from 'vitest';
import { midiClipLink, midiClipSeed } from './midiClipSeed';

describe('midiClipLink', () => {
	it('points at the content of the clip and keeps its trim offset', () => {
		expect(midiClipLink({ offset: 0.5, content_id: 'c1' })).toEqual({
			offset: 0.5,
			contentId: 'c1'
		});
	});
});

describe('midiClipSeed', () => {
	it('copies notes and controls so the new clip does not share them', () => {
		const note = { id: 'n1', pitch: 60, start: 0, duration: 1, velocity: 100 };
		const seed = midiClipSeed({ offset: 0 }, { notes: [note], channel_controls: [] });
		expect(seed.notes).toEqual([note]);
		expect(seed.notes[0]).not.toBe(note);
	});
});
