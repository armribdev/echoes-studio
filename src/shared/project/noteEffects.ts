import type { ChordShape, NoteEffectSettings, ScaleKind } from '../../types/global';

/** What the engine uses for a track that was never configured (ADR-038). */
export const DEFAULT_NOTE_EFFECTS: NoteEffectSettings = {
	transpose: { enabled: false, semitones: 0 },
	scale: { enabled: false, root: 0, scale: 'major' },
	chord: { enabled: false, shape: 'major' }
};

export const MIN_TRANSPOSE_SEMITONES = -24;
export const MAX_TRANSPOSE_SEMITONES = 24;

export const SCALE_ROOT_OPTIONS: { value: number; label: string }[] = [
	'Do',
	'Do♯',
	'Ré',
	'Ré♯',
	'Mi',
	'Fa',
	'Fa♯',
	'Sol',
	'Sol♯',
	'La',
	'La♯',
	'Si'
].map((label, value) => ({ value, label }));

export const SCALE_OPTIONS: { value: ScaleKind; label: string }[] = [
	{ value: 'major', label: 'Majeure' },
	{ value: 'natural_minor', label: 'Mineure naturelle' },
	{ value: 'harmonic_minor', label: 'Mineure harmonique' },
	{ value: 'dorian', label: 'Dorien' },
	{ value: 'phrygian', label: 'Phrygien' },
	{ value: 'lydian', label: 'Lydien' },
	{ value: 'mixolydian', label: 'Mixolydien' },
	{ value: 'pentatonic_major', label: 'Pentatonique majeure' },
	{ value: 'pentatonic_minor', label: 'Pentatonique mineure' },
	{ value: 'blues', label: 'Blues' }
];

export const CHORD_SHAPE_OPTIONS: { value: ChordShape; label: string }[] = [
	{ value: 'octave', label: 'Octave' },
	{ value: 'fifth', label: 'Quinte' },
	{ value: 'major', label: 'Majeur' },
	{ value: 'minor', label: 'Mineur' },
	{ value: 'sus2', label: 'Sus2' },
	{ value: 'sus4', label: 'Sus4' },
	{ value: 'diminished', label: 'Diminué' },
	{ value: 'augmented', label: 'Augmenté' },
	{ value: 'major7', label: 'Majeur 7' },
	{ value: 'minor7', label: 'Mineur 7' },
	{ value: 'dominant7', label: 'Dominante 7' }
];
