import { writable } from 'svelte/store';

export interface TempoState {
	bpm: number;
	numerator: number;
	denominator: number;
}

export const tempo = writable<TempoState>({
	bpm: 120,
	numerator: 4,
	denominator: 4
});
