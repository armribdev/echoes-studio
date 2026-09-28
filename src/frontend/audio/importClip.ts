import { invoke } from '@tauri-apps/api/core';
import { addClip } from '../commands/dispatcher';
import { notify } from '../../stores/status';
import { audioImportProgress } from './importProgress';
import { pendingImports } from './pendingImports';

function fileName(path: string): string {
	return path.split(/[\\/]/).pop() ?? path;
}

export interface ImportAudioOptions {
	trackId: string;
	filePath: string;
	startSec: number;
	/** When omitted or ≤ start, the engine uses the decoded file length. */
	endSec?: number;
	/** Skip duration probe — use when endSec is already known (ghost drop). */
	knownDurationSec?: number;
}

interface PrepareAudioImportResult {
	duration_sec: number;
	sample_rate: number;
}

/**
 * Import an audio file onto a track with optimistic arrange preview.
 * Decode runs on an async Rust command so progressive waveform events can paint.
 */
export async function importAudioToTrack(opts: ImportAudioOptions): Promise<void> {
	const name = fileName(opts.filePath);
	await Promise.all([audioImportProgress.connect(), pendingImports.connect()]);

	let endSec = opts.endSec;
	if (endSec == null || endSec <= opts.startSec) {
		if (opts.knownDurationSec != null && opts.knownDurationSec > 0) {
			endSec = opts.startSec + opts.knownDurationSec;
		} else {
			try {
				const dur = await invoke<number>('get_audio_clip_duration', {
					filePath: opts.filePath,
				});
				endSec = opts.startSec + dur;
			} catch {
				endSec = opts.startSec + 4;
			}
		}
	}

	const pendingId = pendingImports.add({
		trackId: opts.trackId,
		filePath: opts.filePath,
		fileName: name,
		start: opts.startSec,
		end: endSec,
	});
	audioImportProgress.begin(name, opts.filePath);
	notify(`Importation de « ${name} »…`, 'info', 120_000);

	try {
		// Decode off the sync command path so `audio://import-progress` events
		// reach the UI while samples stream in.
		const prepared = await invoke<PrepareAudioImportResult>('prepare_audio_import', {
			filePath: opts.filePath,
		});
		if (prepared?.duration_sec > 0) {
			endSec = opts.startSec + prepared.duration_sec;
		}
		await addClip(opts.trackId, opts.filePath, opts.startSec, endSec);
		notify(`Importé : ${name}`, 'success');
	} catch (e) {
		const msg = e instanceof Error ? e.message : String(e);
		notify(`Erreur import : ${msg}`, 'error');
		throw e;
	} finally {
		pendingImports.remove(pendingId);
		audioImportProgress.end();
	}
}

/** Resolve duration for drag ghosts without importing. */
export async function probeAudioDuration(filePath: string): Promise<number> {
	return invoke<number>('get_audio_clip_duration', { filePath });
}
