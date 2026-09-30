import type { LibraryChordQuality, LibraryChordStep, LibraryMidiNote } from './types';

const QUALITY_INTERVALS: Record<LibraryChordQuality, number[]> = {
	maj: [0, 4, 7],
	min: [0, 3, 7],
	dom7: [0, 4, 7, 10],
	min7: [0, 3, 7, 10],
	maj7: [0, 4, 7, 11],
	sus2: [0, 2, 7],
	sus4: [0, 5, 7],
	dim: [0, 3, 6],
	aug: [0, 4, 8]
};

/** Expand one chord step into block-chord notes (factory voicing, ADR-023 Phase 3). */
export function expandChordStep(step: LibraryChordStep): LibraryMidiNote[] {
	const intervals = QUALITY_INTERVALS[step.quality] ?? QUALITY_INTERVALS.maj;
	return intervals.map((semitone) => ({
		pitch: clampPitch(step.rootPitch + semitone),
		start: step.startBeat,
		duration: Math.max(0.05, step.durationBeats * 0.92),
		velocity: 96
	}));
}

export function expandChordProgression(chords: LibraryChordStep[]): LibraryMidiNote[] {
	return chords.flatMap(expandChordStep);
}

export function transposeNotes(notes: LibraryMidiNote[], semitones: number): LibraryMidiNote[] {
	if (semitones === 0) return notes;
	return notes.map((n) => ({
		...n,
		pitch: clampPitch(n.pitch + semitones)
	}));
}

const ROOT_TO_PC: Record<string, number> = {
	C: 0,
	'C#': 1,
	Db: 1,
	D: 2,
	'D#': 3,
	Eb: 3,
	E: 4,
	F: 5,
	'F#': 6,
	Gb: 6,
	G: 7,
	'G#': 8,
	Ab: 8,
	A: 9,
	'A#': 10,
	Bb: 10,
	B: 11
};

/** Semitone offset so asset keyRoot lands on projectKeyRoot (preview only). */
export function keyTransposeSemitones(
	assetKeyRoot: string | undefined,
	projectKeyRoot: string
): number {
	if (!assetKeyRoot) return 0;
	const from = ROOT_TO_PC[assetKeyRoot];
	const to = ROOT_TO_PC[projectKeyRoot];
	if (from == null || to == null) return 0;
	let delta = to - from;
	if (delta > 6) delta -= 12;
	if (delta < -6) delta += 12;
	return delta;
}

function clampPitch(pitch: number): number {
	return Math.max(0, Math.min(127, Math.round(pitch)));
}
