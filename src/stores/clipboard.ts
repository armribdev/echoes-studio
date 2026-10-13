import { writable } from 'svelte/store';
import type { MidiControlPoint, MidiNote } from '../types/global';

interface ClipboardBase {
	trackId: string;
	durationSec: number;
	/** Seconds from the earliest clip in the copied selection. */
	relativeStart: number;
}

export interface AudioClipboardEntry extends ClipboardBase {
	kind: 'audio';
	filePath: string;
}

export interface MidiClipboardEntry extends ClipboardBase {
	kind: 'midi';
	/** Left-trim offset in seconds. */
	offset: number;
	notes: MidiNote[];
	channelControls: MidiControlPoint[];
}

export type ClipboardEntry = AudioClipboardEntry | MidiClipboardEntry;

export const clipboard = writable<ClipboardEntry[]>([]);
