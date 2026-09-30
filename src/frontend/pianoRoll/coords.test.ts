import { describe, expect, it } from 'vitest';
import type { MidiNote } from '../../types/global';
import {
	beatFromGridX,
	contentEndBeats,
	hitTestNote,
	isResizeHandle,
	pitchFromGridY,
	pitchToYEdit,
	snapBeat,
	visibleBeatsForEdit,
	NOTE_H,
	TOTAL_PITCHES
} from './coords';

const note = (partial: Partial<MidiNote> & Pick<MidiNote, 'id' | 'pitch' | 'start'>): MidiNote => ({
	duration: 1,
	velocity: 100,
	...partial
});

describe('snapBeat / pitch mapping', () => {
	it('snaps beats down to the quantize grid', () => {
		expect(snapBeat(1.9, 0.25)).toBe(1.75);
		expect(snapBeat(2, 0.25)).toBe(2);
	});

	it('maps pitch to edit Y and back from grid Y', () => {
		expect(pitchToYEdit(127)).toBe(0);
		expect(pitchToYEdit(0)).toBe((TOTAL_PITCHES - 1) * NOTE_H);
		expect(pitchFromGridY(0, NOTE_H)).toBe(127);
		expect(pitchFromGridY(NOTE_H, NOTE_H)).toBe(126);
	});

	it('converts grid X to beats', () => {
		expect(beatFromGridX(100, 50)).toBe(2);
	});
});

describe('hitTestNote / isResizeHandle', () => {
	const notes = [
		note({ id: 'a', pitch: 60, start: 0, duration: 2 }),
		note({ id: 'b', pitch: 64, start: 1, duration: 1 })
	];

	it('finds the note under a beat/pitch and ignores edges at the end', () => {
		expect(hitTestNote(notes, 0.5, 60)?.id).toBe('a');
		expect(hitTestNote(notes, 2, 60)).toBeUndefined();
		expect(hitTestNote(notes, 1.2, 64)?.id).toBe('b');
	});

	it('detects the resize handle near the note end', () => {
		const n = notes[0];
		const zoom = 40;
		expect(isResizeHandle(n, 2, zoom)).toBe(true);
		expect(isResizeHandle(n, 0.5, zoom)).toBe(false);
	});
});

describe('contentEndBeats / visibleBeatsForEdit', () => {
	it('extends past clip duration when notes overrun', () => {
		const notes = [note({ id: 'a', pitch: 60, start: 14, duration: 4 })];
		expect(contentEndBeats(notes, 8)).toBe(18);
		expect(visibleBeatsForEdit(notes, 8, 2, 16)).toBe(20);
		expect(visibleBeatsForEdit([], 4, 8, 16)).toBe(16);
	});
});
