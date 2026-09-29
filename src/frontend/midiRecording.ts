import { get } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { addMidiClip, play } from './commands/dispatcher';
import {
	MIDI_RECORDING_FINISHED_EVENT,
	startMidiRecording,
	stopMidiRecording,
	type RecordedMidiClip,
} from '../lib/midiBackend';
import { armedTrackId } from '../stores/armedTrack';
import { isMidiRecording } from '../stores/midiRecording';
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
	if (!get(armedTrackId)) {
		notify('Arme une piste pour enregistrer', 'error');
		return;
	}
	try {
		await startMidiRecording();
		isMidiRecording.set(true);
		if (!get(transport).isPlaying) await play();
	} catch (error) {
		await stopMidiRecording().catch(console.error);
		isMidiRecording.set(false);
		notify(`Erreur enregistrement MIDI : ${errorText(error)}`, 'error');
	}
}

async function addRecordedClip(clip: RecordedMidiClip): Promise<void> {
	await addMidiClip(clip.trackId, clip.start, clip.end, { notes: clip.notes });
}

/** Adds the clip when a recording ends, whichever way it ended (record, pause, stop). */
export function connectMidiRecording(): Promise<UnlistenFn> {
	return listen<RecordedMidiClip | null>(MIDI_RECORDING_FINISHED_EVENT, ({ payload }) => {
		isMidiRecording.set(false);
		if (!payload) {
			notify('Aucune note enregistrée', 'info');
			return;
		}
		addRecordedClip(payload).catch((error) =>
			notify(`Erreur ajout du clip enregistré : ${errorText(error)}`, 'error')
		);
	});
}
