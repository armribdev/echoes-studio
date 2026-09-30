import { DEFAULT_MIDI_GRID_BEATS } from './midiGrid';

export const DEFAULT_KEY_ROOT = 'C';
export const DEFAULT_KEY_MODE = 'major';
export const DEFAULT_SWING_PERCENT = 0;
export const DEFAULT_MIDI_PPQ = 960;

export const KEY_ROOT_OPTIONS = [
	'C',
	'C#',
	'D',
	'D#',
	'E',
	'F',
	'F#',
	'G',
	'G#',
	'A',
	'A#',
	'B'
] as const;

export type KeyRoot = (typeof KEY_ROOT_OPTIONS)[number];

export const KEY_MODE_OPTIONS = [
	{ value: 'major', label: 'Majeur' },
	{ value: 'minor', label: 'Mineur' }
] as const;

export const MIDI_PPQ_OPTIONS = [
	{ value: 480, label: '480' },
	{ value: 960, label: '960' },
	{ value: 1920, label: '1920' }
] as const;

export function keySignatureLabel(root: string, mode: string): string {
	const m = KEY_MODE_OPTIONS.find((o) => o.value === mode);
	return `${root} ${m?.label ?? mode}`;
}

export interface ProjectMusicalFields {
	bpm: number;
	numerator: number;
	denominator: number;
	midi_grid_beats: number;
	key_root: string;
	key_mode: string;
	swing_percent: number;
	midi_ppq: number;
	midi_grid_use_triplets: boolean;
}

export function musicalFieldsFromDto(
	dto: Pick<
		ProjectMusicalFields,
		| 'bpm'
		| 'numerator'
		| 'denominator'
		| 'midi_grid_beats'
		| 'key_root'
		| 'key_mode'
		| 'swing_percent'
		| 'midi_ppq'
		| 'midi_grid_use_triplets'
	>
): ProjectMusicalFields {
	return {
		bpm: dto.bpm,
		numerator: dto.numerator,
		denominator: dto.denominator,
		midi_grid_beats: dto.midi_grid_beats ?? DEFAULT_MIDI_GRID_BEATS,
		key_root: dto.key_root ?? DEFAULT_KEY_ROOT,
		key_mode: dto.key_mode ?? DEFAULT_KEY_MODE,
		swing_percent: dto.swing_percent ?? DEFAULT_SWING_PERCENT,
		midi_ppq: dto.midi_ppq ?? DEFAULT_MIDI_PPQ,
		midi_grid_use_triplets: dto.midi_grid_use_triplets ?? false
	};
}
