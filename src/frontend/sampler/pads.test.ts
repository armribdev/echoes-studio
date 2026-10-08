import { describe, expect, it } from 'vitest';
import type { SamplerPad } from '../../types/global';
import { noteName, padLabel, padRows } from './pads';

function pad(note: number, sample_path: string | null = null): SamplerPad {
	return {
		note,
		sample_path,
		gain_db: 0,
		pan: 0,
		tune_semitones: 0,
		mode: 'one_shot',
		attack_ms: 0,
		release_ms: 10,
		choke_group: 0,
		missing: false
	};
}

describe('sampler pads', () => {
	it('names notes like the piano roll', () => {
		expect(noteName(60)).toBe('C4');
		expect(noteName(36)).toBe('C2');
		expect(noteName(42)).toBe('F#2');
	});

	it('labels a pad by its file, or its note when empty', () => {
		expect(padLabel(pad(36))).toBe('C2');
		expect(padLabel(pad(36, 'C:\\Samples\\Kick 01.wav'))).toBe('Kick 01');
		expect(padLabel(pad(38, '/home/me/snare.flac'))).toBe('snare');
	});

	it('puts pad 1 bottom-left', () => {
		expect(padRows(16)).toEqual([
			[12, 13, 14, 15],
			[8, 9, 10, 11],
			[4, 5, 6, 7],
			[0, 1, 2, 3]
		]);
	});
});
