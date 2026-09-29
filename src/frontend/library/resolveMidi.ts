import type { LibraryAsset, LibraryMidiNote } from './types';
import { isMidiLibraryKind } from './types';
import { expandChordProgression, keyTransposeSemitones, transposeNotes } from './chordExpand';

/** Resolve playable notes for a MIDI/chord asset (optionally transposed for preview). */
export function resolveLibraryMidiNotes(
	asset: LibraryAsset,
	opts?: { projectKeyRoot?: string; transposeForPreview?: boolean },
): LibraryMidiNote[] {
	if (!isMidiLibraryKind(asset.kind)) return [];
	let notes: LibraryMidiNote[];
	if (asset.kind === 'chord_progression' && asset.chords?.length) {
		notes = expandChordProgression(asset.chords);
	} else {
		notes = asset.notes ?? [];
	}
	if (opts?.transposeForPreview && opts.projectKeyRoot) {
		const delta = keyTransposeSemitones(asset.keyRoot, opts.projectKeyRoot);
		notes = transposeNotes(notes, delta);
	}
	return notes;
}

export function beatsToSeconds(beats: number, bpm: number): number {
	return (beats * 60) / Math.max(1, bpm);
}

export function libraryAssetLengthSec(asset: LibraryAsset, bpm: number): number | null {
	if (asset.lengthBeats != null && asset.lengthBeats > 0) {
		return beatsToSeconds(asset.lengthBeats, bpm);
	}
	if (asset.durationSec != null && asset.durationSec > 0) return asset.durationSec;
	return null;
}
