/** Subdivision de grille en unités de noire (1 = noire, 0.25 = double-croche). */
export const DEFAULT_MIDI_GRID_BEATS = 0.25;

export interface MidiGridOption {
	label: string;
	beats: number;
}

export const MIDI_GRID_OPTIONS: MidiGridOption[] = [
	{ label: '1/1', beats: 4 },
	{ label: '1/2', beats: 2 },
	{ label: '1/4', beats: 1 },
	{ label: '1/8', beats: 0.5 },
	{ label: '1/16', beats: 0.25 },
	{ label: '1/32', beats: 0.125 }
];

export function midiGridLabel(beats: number): string {
	return MIDI_GRID_OPTIONS.find((o) => o.beats === beats)?.label ?? `1/${Math.round(1 / beats)}`;
}

export function normalizeMidiGridBeats(beats: number): number {
	const match = MIDI_GRID_OPTIONS.find((o) => Math.abs(o.beats - beats) < 1e-6);
	return match?.beats ?? DEFAULT_MIDI_GRID_BEATS;
}
