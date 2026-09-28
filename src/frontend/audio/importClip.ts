import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { addClip } from '../commands/dispatcher';
import { notify } from '../../stores/status';
import { audioImportProgress } from './importProgress';
import { pendingImports } from './pendingImports';
import { forgetClipUiKey } from './clipDisplay';
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

interface PcmReadyPayload {
	file_path: string;
	decoded_duration_sec: number;
	sample_rate: number;
	progress: number;
	complete: boolean;
	sample_count?: number;
	cached?: boolean;
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

function createPcmReadyGate(filePath: string, timeoutMs = 120_000) {
	const target = normPath(filePath);
	let resolve!: (p: PcmReadyPayload) => void;
	let reject!: (e: Error) => void;
	const promise = new Promise<PcmReadyPayload>((res, rej) => {
		resolve = res;
		reject = rej;
	});

	let unlisten: UnlistenFn | null = null;
	let settled = false;
	const timer = setTimeout(() => {
		if (settled) return;
		settled = true;
		unlisten?.();
		reject(new Error('Timed out waiting for audio PCM'));
	}, timeoutMs);

	const ready = listen<PcmReadyPayload>('audio://import-pcm-ready', ({ payload }) => {
		if (settled) return;
		if (normPath(payload.file_path) !== target) return;
		settled = true;
		clearTimeout(timer);
		unlisten?.();
		resolve(payload);
	}).then((fn) => {
		unlisten = fn;
	});

	return { ready, promise };
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
 * Progressive PCM prefixes allow arrange playback before decode completes (ADR-024).
 */
export async function importAudioToTrack(opts: ImportAudioOptions): Promise<void> {
	const name = fileName(opts.filePath);
	const key = normPath(opts.filePath);
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
	const entry: InFlightImport = { pendingId, cancelled: false };
	inFlight.set(key, entry);
	audioImportProgress.begin(name, opts.filePath);

	const pcmGate = createPcmReadyGate(opts.filePath);
	await pcmGate.ready;
	const preparePromise = invoke<PrepareAudioImportResult>('prepare_audio_import', {
		filePath: opts.filePath,
	});

	try {
		await pcmGate.promise;
		if (entry.cancelled) {
			await preparePromise.catch(() => {});
			return;
		}

		const placement = pendingImports.get(pendingId);
		const trackId = placement?.trackId ?? opts.trackId;
		const startSec = placement?.start ?? opts.startSec;
		const clipEnd = placement?.end ?? endSec;

		const result = await addClip(trackId, opts.filePath, startSec, clipEnd);
		if (entry.cancelled) {
			await preparePromise.catch(() => {});
			return;
		}

		const engineClipId = clipIdFromResult(result);
		if (engineClipId) {
			pendingImports.attachEngineClip(pendingId, engineClipId);
		}

		const prepared = await preparePromise;
		if (entry.cancelled) return;

		if (prepared?.duration_sec > 0) {
			const latest = pendingImports.get(pendingId);
			const s = latest?.start ?? startSec;
			pendingImports.updatePlacement(pendingId, {
				start: s,
				end: s + prepared.duration_sec,
			});
		}

		pendingImports.markSettled(pendingId);
		if (entry.cancelled) return;
		notify(`Importé : ${name}`, 'success');
	} catch (e) {
		if (entry.cancelled) return;
		const msg = e instanceof Error ? e.message : String(e);
		if (/cancelled/i.test(msg)) return;
		notify(`Erreur import : ${msg}`, 'error');
		throw e;
	} finally {
		inFlight.delete(key);
		if (!entry.cancelled) {
			audioImportProgress.end();
		}
	}
}

/** Resolve duration for drag ghosts without importing. */
export async function probeAudioDuration(filePath: string): Promise<number> {
	return invoke<number>('get_audio_clip_duration', { filePath });
}
