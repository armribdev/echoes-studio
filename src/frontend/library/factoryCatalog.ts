import type { LibraryAsset, LibraryChordStep, LibraryMidiNote } from './types';

function n(pitch: number, start: number, duration: number, velocity = 100): LibraryMidiNote {
	return { pitch, start, duration, velocity };
}

function chord(
	symbol: string,
	rootPitch: number,
	quality: LibraryChordStep['quality'],
	startBeat: number,
	durationBeats: number,
): LibraryChordStep {
	return { symbol, rootPitch, quality, startBeat, durationBeats };
}

function factoryChord(
	id: string,
	name: string,
	keyRoot: string,
	lengthBeats: number,
	chords: LibraryChordStep[],
	tags: string[] = [],
): LibraryAsset {
	return {
		id: `factory://chords/${id}`,
		path: `factory://chords/${id}`,
		name,
		kind: 'chord_progression',
		extension: 'chd',
		lengthBeats,
		keyRoot,
		chords,
		origin: 'factory',
		tags,
	};
}

function factoryMidi(
	id: string,
	name: string,
	keyRoot: string | undefined,
	lengthBeats: number,
	notes: LibraryMidiNote[],
	tags: string[] = [],
): LibraryAsset {
	return {
		id: `factory://midi/${id}`,
		path: `factory://midi/${id}`,
		name,
		kind: 'midi_pattern',
		extension: 'mid',
		lengthBeats,
		keyRoot,
		notes,
		origin: 'factory',
		tags,
	};
}

/** Built-in musical content for Phase 3 (factory MIDI first, then chord expand). */
export const FACTORY_LIBRARY_ASSETS: LibraryAsset[] = [
	factoryChord('ii-v-i-c', 'II–V–I (C majeur)', 'C', 8, [
		chord('Dm7', 62, 'min7', 0, 2),
		chord('G7', 55, 'dom7', 2, 2),
		chord('Cmaj7', 60, 'maj7', 4, 4),
	]),
	factoryChord('vi-iv-i-v-c', 'VI–IV–I–V (C)', 'C', 8, [
		chord('Am', 57, 'min', 0, 2),
		chord('F', 53, 'maj', 2, 2),
		chord('C', 48, 'maj', 4, 2),
		chord('G', 55, 'maj', 6, 2),
	]),
	factoryChord('i-v-vi-iv-am', 'I–V–VI–IV (Am)', 'A', 8, [
		chord('Am', 57, 'min', 0, 2),
		chord('E', 52, 'maj', 2, 2),
		chord('F', 53, 'maj', 4, 2),
		chord('Dm', 50, 'min', 6, 2),
	]),
	factoryChord('i-bvii-bvi-v-em', 'I–bVII–bVI–V (Em)', 'E', 8, [
		chord('Em', 52, 'min', 0, 2),
		chord('D', 50, 'maj', 2, 2),
		chord('C', 48, 'maj', 4, 2),
		chord('B7', 47, 'dom7', 6, 2),
	]),
	factoryChord('blues-c', 'Blues 12 measures (C)', 'C', 12, [
		chord('C7', 48, 'dom7', 0, 4),
		chord('F7', 53, 'dom7', 4, 2),
		chord('C7', 48, 'dom7', 6, 2),
		chord('G7', 55, 'dom7', 8, 1),
		chord('F7', 53, 'dom7', 9, 1),
		chord('C7', 48, 'dom7', 10, 2),
	]),

	factoryMidi(
		'arp-cmaj-up',
		'Arpège C majeur ↑',
		'C',
		4,
		[n(60, 0, 0.45), n(64, 0.5, 0.45), n(67, 1, 0.45), n(72, 1.5, 0.45), n(67, 2, 0.45), n(64, 2.5, 0.45), n(60, 3, 0.9)],
	),
	factoryMidi(
		'bass-c-root-fifth',
		'Basse root–fifth (C)',
		'C',
		4,
		[n(36, 0, 0.9), n(43, 1, 0.9), n(36, 2, 0.9), n(43, 3, 0.9)],
	),
	factoryMidi(
		'melody-pentatonic-c',
		'Mélodie pentatonique C',
		'C',
		8,
		[
			n(72, 0, 0.9),
			n(74, 1, 0.45),
			n(76, 1.5, 0.45),
			n(79, 2, 0.9),
			n(76, 3, 0.9),
			n(74, 4, 0.45),
			n(72, 4.5, 0.45),
			n(67, 5, 1.8),
			n(72, 7, 0.9),
		],
	),
	factoryMidi(
		'pads-cmaj7',
		'Pad Cmaj7 (tenues)',
		'C',
		8,
		[n(48, 0, 7.5, 80), n(55, 0, 7.5, 70), n(60, 0, 7.5, 75), n(64, 0, 7.5, 70), n(67, 0, 7.5, 65)],
	),

	factoryMidi(
		'drums-basic-4',
		'Batterie 4/4 basique',
		undefined,
		4,
		[
			n(36, 0, 0.2, 110),
			n(42, 0, 0.15, 80),
			n(42, 0.5, 0.15, 60),
			n(38, 1, 0.2, 100),
			n(42, 1, 0.15, 80),
			n(42, 1.5, 0.15, 60),
			n(36, 2, 0.2, 110),
			n(42, 2, 0.15, 80),
			n(42, 2.5, 0.15, 60),
			n(38, 3, 0.2, 100),
			n(42, 3, 0.15, 80),
			n(42, 3.5, 0.15, 60),
		],
		['drums'],
	),
	factoryMidi(
		'drums-kick-hat',
		'Kick + hats',
		undefined,
		4,
		[
			n(36, 0, 0.25, 115),
			n(42, 0, 0.12, 70),
			n(42, 0.5, 0.12, 55),
			n(42, 1, 0.12, 70),
			n(42, 1.5, 0.12, 55),
			n(36, 2, 0.25, 115),
			n(42, 2, 0.12, 70),
			n(42, 2.5, 0.12, 55),
			n(42, 3, 0.12, 70),
			n(42, 3.5, 0.12, 55),
		],
		['drums'],
	),
	factoryMidi(
		'drums-break-snare',
		'Break snare',
		undefined,
		4,
		[
			n(36, 0, 0.2, 110),
			n(38, 0.75, 0.15, 90),
			n(38, 1, 0.2, 105),
			n(36, 2, 0.2, 110),
			n(38, 2.5, 0.15, 85),
			n(38, 3, 0.15, 95),
			n(38, 3.5, 0.2, 110),
		],
		['drums'],
	),
];

export function isFactoryAssetId(id: string): boolean {
	return id.startsWith('factory://');
}
