import { writable } from 'svelte/store';
import type { RecordedMidiClip, RecordedNote } from '../lib/midiBackend';

/** True while the armed track is recording hardware MIDI notes. Session-only UI state. */
export const isMidiRecording = writable(false);

/**
 * What the arrange view draws while a recording runs, then until the real clip
 * takes over: the notes received so far, or the finished clip once it ended.
 */
export interface MidiRecordingPreview {
	trackId: string;
	/** Timeline seconds where the recording started. */
	start: number;
	events: RecordedNote[];
	finished: RecordedMidiClip | null;
}

export const midiRecordingPreview = writable<MidiRecordingPreview | null>(null);
