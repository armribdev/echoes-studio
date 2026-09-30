import { writable } from 'svelte/store';
import type { MidiNote } from '../types/global';

/** Note copiée, positionnée par rapport à la note la plus à gauche. */
export interface CopiedNote {
	pitch: number;
	relativeStart: number;
	duration: number;
	velocity: number;
}

export const noteClipboard = writable<CopiedNote[]>([]);

export function copyMidiNotes(notes: MidiNote[]): void {
	if (notes.length === 0) return;
	let origin = notes[0].start;
	for (const note of notes) origin = Math.min(origin, note.start);
	noteClipboard.set(
		notes.map((note) => ({
			pitch: note.pitch,
			relativeStart: note.start - origin,
			duration: note.duration,
			velocity: note.velocity
		}))
	);
}
