import { writable } from 'svelte/store';
import type { RecordedAudioClip } from '../lib/audioRecordingBackend';

/** True while the armed audio track is recording CPAL input. Session-only UI state. */
export const isAudioRecording = writable(false);

/**
 * Arrange preview while a take runs, then until `ADD_CLIP` lands (anti-flash).
 */
export interface AudioRecordingPreview {
	trackId: string;
	/** Timeline seconds where the recording started. */
	start: number;
	finished: RecordedAudioClip | null;
	/** Live waveform of the take, 512 frames per entry. */
	peaks: [number, number][];
	sampleRate: number;
}

export const audioRecordingPreview = writable<AudioRecordingPreview | null>(null);

/** Session software-monitoring flag (ADR-027), mirrored from the backend. */
export const inputMonitorEnabled = writable(false);
