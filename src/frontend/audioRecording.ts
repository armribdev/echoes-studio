import { get } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { addClip, play } from './commands/dispatcher';
import {
	AUDIO_RECORDING_FINISHED_EVENT,
	getAudioRecordingPeaks,
	startAudioRecording,
	stopAudioRecording,
	type RecordedAudioClip
} from '../lib/audioRecordingBackend';
import { armedTrackId } from '../stores/armedTrack';
import { countInEnabled } from '../stores/metronome';
import { isAudioRecording, audioRecordingPreview } from '../stores/audioRecording';
import { engineProject } from './store/engineStore';
import { notify } from '../stores/status';
import { transport } from '../stores/transport';
import { isMidiRecording } from '../stores/midiRecording';
import { toggleMidiRecording } from './midiRecording';

function errorText(error: unknown): string {
	return (error as Error)?.message ?? String(error);
}

function armedTrackHasInstrument(trackId: string): boolean {
	const track = get(engineProject).tracks.find((t) => t.id === trackId);
	return Boolean(track?.instrument);
}

/**
 * ● button: MIDI on instrument tracks, audio otherwise (ADR-022 / ADR-027).
 */
export async function toggleRecording(): Promise<void> {
	if (get(isMidiRecording)) {
		await toggleMidiRecording();
		return;
	}
	if (get(isAudioRecording)) {
		await stopAudioRecording();
		return;
	}

	const trackId = get(armedTrackId);
	if (!trackId) {
		notify('Arme une piste pour enregistrer', 'error');
		return;
	}

	if (armedTrackHasInstrument(trackId)) {
		await toggleMidiRecording();
		return;
	}

	try {
		const start = await startAudioRecording(get(countInEnabled) ? 1 : 0);
		audioRecordingPreview.set({ trackId, start, finished: null, peaks: [], sampleRate: 48000 });
		isAudioRecording.set(true);
		if (!get(transport).isPlaying) await play();
	} catch (error) {
		await stopAudioRecording().catch(console.error);
		isAudioRecording.set(false);
		audioRecordingPreview.set(null);
		notify(`Erreur enregistrement audio : ${errorText(error)}`, 'error');
	}
}

const PEAKS_POLL_INTERVAL_MS = 80;

async function appendRecordingPeaks(): Promise<void> {
	const preview = get(audioRecordingPreview);
	if (!preview || preview.finished) return;
	const batch = await getAudioRecordingPeaks(preview.peaks.length);
	if (!batch || batch.peaks.length === 0) return;
	audioRecordingPreview.update((current) =>
		current && !current.finished && current.peaks.length === preview.peaks.length
			? {
					...current,
					sampleRate: batch.sampleRate,
					peaks: current.peaks.concat(batch.peaks)
				}
			: current
	);
}

function startPeaksPolling(): () => void {
	let polling = false;
	const timer = setInterval(() => {
		if (polling) return;
		polling = true;
		appendRecordingPeaks()
			.catch(console.error)
			.finally(() => (polling = false));
	}, PEAKS_POLL_INTERVAL_MS);
	return () => clearInterval(timer);
}

function nextFrame(): Promise<void> {
	return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

async function addRecordedClip(clip: RecordedAudioClip): Promise<void> {
	audioRecordingPreview.update((preview) => preview && { ...preview, finished: clip });
	try {
		await addClip(clip.trackId, clip.filePath, clip.start, clip.end);
		await nextFrame();
		await nextFrame();
	} finally {
		audioRecordingPreview.set(null);
	}
}

export async function connectAudioRecording(): Promise<UnlistenFn> {
	let stopPolling: (() => void) | null = null;
	const unsubscribeRecording = isAudioRecording.subscribe((recording) => {
		stopPolling?.();
		stopPolling = recording ? startPeaksPolling() : null;
	});
	const unlistenFinished = await listen<RecordedAudioClip | null>(
		AUDIO_RECORDING_FINISHED_EVENT,
		({ payload }) => {
			isAudioRecording.set(false);
			if (!payload) {
				audioRecordingPreview.set(null);
				notify('Aucun audio enregistré', 'info');
				return;
			}
			addRecordedClip(payload).catch((error) =>
				notify(`Erreur ajout du clip enregistré : ${errorText(error)}`, 'error')
			);
		}
	);
	return () => {
		unsubscribeRecording();
		stopPolling?.();
		unlistenFinished();
	};
}
