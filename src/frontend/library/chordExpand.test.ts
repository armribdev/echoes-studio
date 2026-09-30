import { describe, expect, it } from 'vitest';
import {
	expandChordProgression,
	expandChordStep,
	keyTransposeSemitones,
	transposeNotes
} from './chordExpand';
import type { LibraryChordStep } from './types';

const cMaj: LibraryChordStep = {
	symbol: 'C',
	rootPitch: 60,
	quality: 'maj',
	startBeat: 0,
	durationBeats: 4
};

describe('expandChordStep / expandChordProgression', () => {
	it('expands major and minor qualities to factory voicings', () => {
		expect(expandChordStep(cMaj).map((n) => n.pitch)).toEqual([60, 64, 67]);
		expect(expandChordStep({ ...cMaj, quality: 'min', symbol: 'Cm' }).map((n) => n.pitch)).toEqual([
			60, 63, 67
		]);
		expect(expandChordStep({ ...cMaj, quality: 'dom7', symbol: 'C7' }).map((n) => n.pitch)).toEqual(
			[60, 64, 67, 70]
		);
	});

	it('uses slightly shortened note durations and maps start beats', () => {
		const notes = expandChordStep({ ...cMaj, startBeat: 4, durationBeats: 2 });
		expect(notes.every((n) => n.start === 4)).toBe(true);
		expect(notes[0].duration).toBeCloseTo(1.84, 5);
		expect(notes[0].velocity).toBe(96);
	});

	it('flattens a progression in chronological order', () => {
		const notes = expandChordProgression([
			cMaj,
			{ ...cMaj, symbol: 'F', rootPitch: 65, startBeat: 4 }
		]);
		expect(notes).toHaveLength(6);
		expect(notes.slice(0, 3).every((n) => n.start === 0)).toBe(true);
		expect(notes.slice(3).every((n) => n.start === 4)).toBe(true);
	});
});

describe('transposeNotes / keyTransposeSemitones', () => {
	it('transposes pitches within MIDI range', () => {
		const notes = expandChordStep(cMaj);
		expect(transposeNotes(notes, 0)).toBe(notes);
		expect(transposeNotes(notes, 2).map((n) => n.pitch)).toEqual([62, 66, 69]);
		expect(transposeNotes([{ pitch: 126, start: 0, duration: 1, velocity: 100 }], 5)[0].pitch).toBe(
			127
		);
	});

	it('picks the shortest path between key roots', () => {
		expect(keyTransposeSemitones(undefined, 'C')).toBe(0);
		expect(keyTransposeSemitones('C', 'G')).toBe(-5);
		expect(keyTransposeSemitones('C', 'D')).toBe(2);
		expect(keyTransposeSemitones('Bb', 'C')).toBe(2);
		expect(keyTransposeSemitones('X', 'C')).toBe(0);
	});
});
