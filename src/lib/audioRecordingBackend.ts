import { invoke } from '@tauri-apps/api/core';

/** Backend event fired when an audio recording ends. Payload: `RecordedAudioClip | null`. */
export const AUDIO_RECORDING_FINISHED_EVENT = 'audio-recording-finished';

/** Mirror of `audio_recording::RecordedAudioClip`. */
export interface RecordedAudioClip {
	trackId: string;
	/** Timeline seconds. */
	start: number;
	end: number;
	filePath: string;
}

/**
 * Returns the timeline position (seconds) where the recording starts. From a
 * stopped transport, `countInBars` bars of click play before it moves.
 */
export async function startAudioRecording(countInBars: number): Promise<number> {
	return invoke<number>('start_audio_recording', { countInBars });
}

/** The clip arrives through `AUDIO_RECORDING_FINISHED_EVENT`. */
export async function stopAudioRecording(): Promise<void> {
	await invoke<void>('stop_audio_recording');
}

/** Mirror of `audio_recording::RecordingPeaks`. */
export interface RecordingPeaks {
	sampleRate: number;
	/** `[min, max]` per 512-frame chunk, only the chunks after `from`. */
	peaks: [number, number][];
}

/** `null` once no take is running. */
export async function getAudioRecordingPeaks(from: number): Promise<RecordingPeaks | null> {
	return invoke<RecordingPeaks | null>('get_audio_recording_peaks', { from });
}

export async function setInputMonitorEnabled(enabled: boolean): Promise<void> {
	await invoke<void>('set_input_monitor_enabled', { enabled });
}

export async function getInputMonitorEnabled(): Promise<boolean> {
	return invoke<boolean>('get_input_monitor_enabled');
}
