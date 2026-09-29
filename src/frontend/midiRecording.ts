import { get } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { addMidiClip, play } from './commands/dispatcher';
import {
	MIDI_RECORDING_FINISHED_EVENT,
	MIDI_RECORDING_NOTE_EVENT,
	startMidiRecording,
	stopMidiRecording,
	type RecordedMidiClip,
	type RecordedNote,
} from '../lib/midiBackend';
import { armedTrackId } from '../stores/armedTrack';
import { isMidiRecording, midiRecordingPreview } from '../stores/midiRecording';
import { notify } from '../stores/status';
import { transport } from '../stores/transport';

function errorText(error: unknown): string {
	return (error as Error)?.message ?? String(error);
}

export async function toggleMidiRecording(): Promise<void> {
	if (get(isMidiRecording)) {
		await stopMidiRecording();
		return;
	}
	const trackId = get(armedTrackId);
	if (!trackId) {
		notify('Arme une piste pour enregistrer', 'error');
		return;
	}
	try {
		const start = await startMidiRecording();
		midiRecordingPreview.set({ trackId, start, events: [], finished: null });
		isMidiRecording.set(true);
		if (!get(transport).isPlaying) await play();
	} catch (error) {
		await stopMidiRecording().catch(console.error);
		isMidiRecording.set(false);
		midiRecordingPreview.set(null);
		notify(`Erreur enregistrement MIDI : ${errorText(error)}`, 'error');
	}
}

function nextFrame(): Promise<void> {
	return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

/**
 * The preview freezes on the final clip and stays until the real one is in the
 * store, so the swap does not flash (same idea as the audio import handoff).
 */
async function addRecordedClip(clip: RecordedMidiClip): Promise<void> {
	midiRecordingPreview.update((preview) => preview && { ...preview, finished: clip });
	try {
		await addMidiClip(clip.trackId, clip.start, clip.end, { notes: clip.notes });
		await nextFrame();
		await nextFrame();
	} finally {
		midiRecordingPreview.set(null);
	}
}

/**
 * Follows a recording from the backend: notes grow the preview as they are
 * played, and the clip is added when it ends, whichever way it ended (record,
 * pause, stop).
 */
export async function connectMidiRecording(): Promise<UnlistenFn> {
	const unlistenNotes = await listen<RecordedNote>(MIDI_RECORDING_NOTE_EVENT, ({ payload }) => {
		midiRecordingPreview.update(
			(preview) => preview && { ...preview, events: [...preview.events, payload] }
		);
	});
	const unlistenFinished = await listen<RecordedMidiClip | null>(
		MIDI_RECORDING_FINISHED_EVENT,
		({ payload }) => {
			isMidiRecording.set(false);
			if (!payload) {
				midiRecordingPreview.set(null);
				notify('Aucune note enregistrée', 'info');
				return;
			}
			addRecordedClip(payload).catch((error) =>
				notify(`Erreur ajout du clip enregistré : ${errorText(error)}`, 'error')
			);
		}
	);
	return () => {
		unlistenNotes();
		unlistenFinished();
	};
}
