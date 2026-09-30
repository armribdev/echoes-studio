import { describe, expect, it } from 'vitest';
import type { Track } from '../types/global';
import { getSnapSubdivisionSec, snapPosition } from './snapUtils';

const tempo = { bpm: 120, numerator: 4 };

function trackWithClips(clips: { id: string; start: number; end: number }[]): Track {
	return {
		id: 't1',
		name: 'T',
		volume: 80,
		clips: clips.map((c) => ({
			...c,
			offset: 0,
			file_path: '',
			sample_rate: 48000,
			peaks_64: [],
			peaks_512: [],
			peaks_4096: []
		}))
	} as Track;
}

describe('getSnapSubdivisionSec', () => {
	it('adapts subdivision to zoom while respecting project grid', () => {
		const fine = getSnapSubdivisionSec(tempo, 400, 0.25);
		expect(fine).toBeCloseTo(0.125, 6);

		const coarse = getSnapSubdivisionSec(tempo, 5, 0.25);
		expect(coarse).toBeGreaterThanOrEqual(0.25);
	});
});

describe('snapPosition', () => {
	it('bypasses snapping when Alt is held', () => {
		expect(snapPosition(1.23, true, tempo, 100, 0.25, [])).toBe(1.23);
	});

	it('snaps to the nearest grid or clip edge within the pixel threshold', () => {
		const tracks = [trackWithClips([{ id: 'c1', start: 2, end: 4 }])];
		expect(snapPosition(2.02, false, tempo, 200, 0.25, tracks)).toBe(2);

		const offGrid = [trackWithClips([{ id: 'c1', start: 2.07, end: 3.07 }])];
		expect(snapPosition(2.08, false, tempo, 200, 0.25, offGrid)).toBe(2.07);
		expect(snapPosition(2.08, false, tempo, 200, 0.25, offGrid, 'c1')).toBe(2.125);
	});
});
