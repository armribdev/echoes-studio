import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import type { Track } from '../types/global';
import type { Preferences } from '../stores/preferences';
import { notify } from '../stores/status';

export type ExportFormat = 'wav' | 'mp3';

export async function exportMixdown(
	tracks: Track[],
	preferences: Preferences,
	format: ExportFormat = 'wav'
): Promise<void> {
	let totalDuration = 0;
	for (const track of tracks) {
		for (const clip of track.clips) {
			totalDuration = Math.max(totalDuration, clip.end);
		}
	}
	if (totalDuration <= 0) {
		notify('Aucun clip à exporter', 'error');
		return;
	}

	const isMp3 = format === 'mp3';
	const filePath = await save({
		filters: isMp3
			? [{ name: 'Fichier MP3', extensions: ['mp3'] }]
			: [{ name: 'Fichier WAV', extensions: ['wav'] }],
		defaultPath: isMp3 ? 'mixdown.mp3' : 'mixdown.wav',
	});
	if (!filePath) return;

	notify('Export en cours…', 'info');

	const sampleRate = preferences.audio.sampleRate;

	try {
		if (isMp3) {
			await invoke('export_mp3', {
				path: filePath,
				sampleRate,
				bitrateKbps: preferences.project.exportMp3Bitrate,
			});
		} else {
			await invoke('export_wav', {
				path: filePath,
				sampleRate,
				bitDepth: preferences.project.exportBitDepth,
			});
		}
	} catch (err) {
		notify(`Échec de l'export : ${err}`, 'error');
		return;
	}

	const filename = (filePath as string).split(/[\\/]/).pop() ?? `mixdown.${format}`;
	notify(`Exporté : ${filename}`, 'success');
}
