import { invoke } from '@tauri-apps/api/core';
import { addClip } from '../commands/dispatcher';
import { notify } from '../../stores/status';
import { audioImportProgress } from './importProgress';
import { pendingImports } from './pendingImports';
import { forgetClipUiKey } from './clipDisplay';
import { importMark } from './importDebugMarks';
import type { CommandResult } from '../../shared/protocol/commands';

function fileName(path: string): string {
	return path.split(/[\\/]/).pop() ?? path;
}

function normPath(p: string): string {
	return p.replace(/\\/g, '/').toLowerCase();
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

interface InFlightImport {
	pendingId: string;
	cancelled: boolean;
}

const inFlight = new Map<string, InFlightImport>();

function clipIdFromResult(result: CommandResult): string | null {
	for (const raw of result.events) {
		const payload = (raw as { payload?: { clip?: { id?: string } } }).payload;
		const id = payload?.clip?.id;
		if (id) return id;
	}
	return null;
}

/** Abort an in-flight import (library ■ / stop). Removes the pending clip immediately. */
export async function cancelAudioImport(filePath: string): Promise<void> {
	const key = normPath(filePath);
	const entry = inFlight.get(key);
	if (entry) {
		entry.cancelled = true;
		const slot = pendingImports.get(entry.pendingId);
		if (slot?.engineClipId) forgetClipUiKey(slot.engineClipId);
		pendingImports.remove(entry.pendingId);
		audioImportProgress.end();
	} else {
		pendingImports.removeByPath(filePath);
	}
	try {
		await invoke('cancel_audio_import', { filePath });
	} catch {
		/* best-effort — UI already cleared */
	}
}

/**
 * Import an audio file onto a track with optimistic arrange preview.
 * Decode runs on an async Rust command so progressive waveform events can paint.
 * The pending Clip keeps a stable UI key through ADD_CLIP (anti-flash handoff).
 * Clip frame uses full probed duration from the first paint — waveform fills in.
 */
export async function importAudioToTrack(opts: ImportAudioOptions): Promise<void> {
	const name = fileName(opts.filePath);
	const key = normPath(opts.filePath);
	await Promise.all([audioImportProgress.connect(), pendingImports.connect()]);

	// Resolve full duration before showing the slot so the clip occupies its
	// final arrange width immediately (waveform still streams in).
	let durationSec = opts.knownDurationSec;
	if (durationSec == null || durationSec <= 0) {
		if (opts.endSec != null && opts.endSec > opts.startSec) {
			durationSec = opts.endSec - opts.startSec;
		} else {
			try {
				durationSec = await invoke<number>('get_audio_clip_duration', {
					filePath: opts.filePath,
				});
			} catch {
				durationSec = 4;
			}
		}
	}

	let endSec = opts.startSec + durationSec;
	const pendingId = pendingImports.add({
		trackId: opts.trackId,
		filePath: opts.filePath,
		fileName: name,
		start: opts.startSec,
		end: endSec,
	});
	importMark('01-pending-added');
	const entry: InFlightImport = { pendingId, cancelled: false };
	inFlight.set(key, entry);
	audioImportProgress.begin(name, opts.filePath);

	try {
		importMark('02-prepare-start');
		const prepared = await invoke<PrepareAudioImportResult>('prepare_audio_import', {
			filePath: opts.filePath,
		});
		importMark('03-prepare-done');
		if (entry.cancelled) return;

		// Honour moves made during decode; refine length only if prepare disagrees.
		const placement = pendingImports.get(pendingId);
		const trackId = placement?.trackId ?? opts.trackId;
		let startSec = placement?.start ?? opts.startSec;
		endSec = placement?.end ?? endSec;
		if (prepared?.duration_sec > 0) {
			const refined = startSec + prepared.duration_sec;
			// Avoid sub-pixel frame resize at handoff (canvas buffer clear → flash).
			if (Math.abs(refined - endSec) > 0.05) {
				endSec = refined;
				pendingImports.updatePlacement(pendingId, { start: startSec, end: endSec });
				importMark('04-placement-resized');
			} else {
				endSec = placement?.end ?? refined;
			}
		}

		importMark('05-addClip-start');
		const result = await addClip(trackId, opts.filePath, startSec, endSec);
		importMark('06-addClip-done');
		if (entry.cancelled) return;

		const engineClipId = clipIdFromResult(result);
		if (engineClipId) {
			importMark('07-attach-start');
			pendingImports.attachEngineClip(pendingId, engineClipId);
			importMark('08-attach-done');
			await pendingImports.waitForPaint(pendingId);
			importMark('09-paint-signaled');
			if (entry.cancelled) return;
			pendingImports.markSettled(pendingId);
			importMark('10-settled');
			await new Promise<void>((r) => requestAnimationFrame(() => r()));
			importMark('11-after-raf');
		} else {
			pendingImports.remove(pendingId);
			importMark('08b-no-engine-id-removed');
		}
		if (entry.cancelled) return;
		importMark('12-before-notify');
		notify(`Importé : ${name}`, 'success');
		importMark('13-after-notify');
	} catch (e) {
		importMark('error');
		if (!entry.cancelled) {
			pendingImports.remove(pendingId);
		}
		if (entry.cancelled) return;
		const msg = e instanceof Error ? e.message : String(e);
		if (/cancelled/i.test(msg)) return;
		notify(`Erreur import : ${msg}`, 'error');
		throw e;
	} finally {
		inFlight.delete(key);
		if (!entry.cancelled) {
			audioImportProgress.end();
			importMark('14-progress-end');
		}
	}
}

/** Resolve duration for drag ghosts without importing. */
export async function probeAudioDuration(filePath: string): Promise<number> {
	return invoke<number>('get_audio_clip_duration', { filePath });
}
