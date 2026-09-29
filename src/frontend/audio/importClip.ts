import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { get } from 'svelte/store';
import { addClip, removeClip } from '../commands/dispatcher';
import { notify } from '../../stores/status';
import { selection } from '../../stores/selection';
import { keyboardFocus } from '../../stores/keyboardFocus';
import { projectEventStore } from '../../shared/project/eventStore';
import { audioImportProgress } from './importProgress';
import { pendingImports } from './pendingImports';
import { forgetClipUiKey } from './clipDisplay';
import type { CommandResult } from '../../shared/protocol/commands';

/** Clip selection owns arrange shortcuts (Delete, etc.) — leave library DOM focus. */
function adoptTimelineFocusForImportedClip() {
	keyboardFocus.setTimeline();
	const active = document.activeElement;
	if (active instanceof HTMLElement && active.closest('.library')) {
		active.blur();
	}
}

function fileName(path: string): string {
	return path.split(/[\\/]/).pop() ?? path;
}

function normPath(p: string): string {
	return p.replace(/\\/g, '/').toLowerCase();
}

function pathsMatch(a: string, b: string): boolean {
	return normPath(a) === normPath(b);
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
	trackId: string;
	startSec: number;
	engineClipId?: string;
}

const inFlight = new Map<string, InFlightImport>();

function clipFromResult(result: CommandResult): {
	id: string;
	sample_rate?: number;
	peaks_64?: [number, number][];
	peaks_512?: [number, number][];
	peaks_4096?: [number, number][];
} | null {
	for (const raw of result.events) {
		const payload = (raw as {
			payload?: {
				clip?: {
					id?: string;
					sample_rate?: number;
					peaks_64?: [number, number][];
					peaks_512?: [number, number][];
					peaks_4096?: [number, number][];
				};
			};
		}).payload;
		const clip = payload?.clip;
		if (clip?.id) return { id: clip.id, ...clip };
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

/** Find engine clip created by this import (attached id, or path+start twin). */
function resolveImportEngineClipId(
	trackId: string,
	filePath: string,
	startSec: number,
	knownId?: string,
): string | null {
	if (knownId) return knownId;
	const track = get(projectEventStore).tracks.find((t) => t.id === trackId);
	if (!track) return null;
	const target = normPath(filePath);
	const hit = track.clips.find(
		(c) =>
			normPath(c.file_path ?? '') === target && Math.abs(c.start - startSec) < 0.15,
	);
	return hit?.id ?? null;
}

async function purgeImportEngineClip(
	trackId: string,
	filePath: string,
	startSec: number,
	knownId?: string,
): Promise<void> {
	const clipId = resolveImportEngineClipId(trackId, filePath, startSec, knownId);
	if (!clipId) return;
	forgetClipUiKey(clipId);
	try {
		await removeClip(trackId, clipId);
	} catch {
		/* best-effort — UI pending already gone */
	}
}

/** Abort an in-flight import (library ■ / stop). Removes pending + engine clip. */
export async function cancelAudioImport(filePath: string): Promise<void> {
	const key = normPath(filePath);
	const entry = inFlight.get(key);
	let trackId: string | null = null;
	let startSec = 0;
	let engineClipId: string | undefined;

	if (entry) {
		entry.cancelled = true;
		const slot = pendingImports.get(entry.pendingId);
		trackId = slot?.trackId ?? entry.trackId;
		startSec = slot?.start ?? entry.startSec;
		engineClipId = slot?.engineClipId ?? entry.engineClipId;
		if (slot?.engineClipId) forgetClipUiKey(slot.engineClipId);
		// Only this slot — never removeByPath (would wipe settled siblings).
		pendingImports.remove(entry.pendingId, { force: true });
		audioImportProgress.end();
	} else {
		// Library stop with no local inFlight: cancel unsettled slots only.
		const doomed = get(pendingImports).filter(
			(item) => pathsMatch(item.filePath, filePath) && !item.settled,
		);
		for (const item of doomed) {
			trackId = item.trackId;
			startSec = item.start;
			engineClipId = item.engineClipId;
			if (item.engineClipId) forgetClipUiKey(item.engineClipId);
			pendingImports.remove(item.id, { force: true });
		}
		if (doomed.length === 0) {
			selection.clear();
			try {
				await invoke('cancel_audio_import', { filePath });
			} catch {
				/* best-effort */
			}
			return;
		}
	}

	selection.clear();

	try {
		await invoke('cancel_audio_import', { filePath });
	} catch {
		/* best-effort — UI already cleared */
	}

	if (trackId) {
		await purgeImportEngineClip(trackId, filePath, startSec, engineClipId);
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
	// If no pending sibling had peaks (slot pruned), borrow from an engine twin.
	{
		const slot = pendingImports.get(pendingId);
		if (slot && slot.peaks_512.length === 0 && slot.peaks_4096.length === 0) {
			const target = normPath(opts.filePath);
			for (const track of get(projectEventStore).tracks) {
				const twin = track.clips.find(
					(c) =>
						normPath(c.file_path ?? '') === target &&
						((c.peaks_512?.length ?? 0) > 0 || (c.peaks_4096?.length ?? 0) > 0),
				);
				if (twin) {
					pendingImports.seedPeaks(pendingId, {
						sampleRate: twin.sample_rate,
						peaks_64: twin.peaks_64,
						peaks_512: twin.peaks_512,
						peaks_4096: twin.peaks_4096,
					});
					break;
				}
			}
		}
	}
	selection.setSingle(pendingId);
	adoptTimelineFocusForImportedClip();
	const entry: InFlightImport = {
		pendingId,
		cancelled: false,
		trackId: opts.trackId,
		startSec: opts.startSec,
	};
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
		entry.trackId = trackId;
		entry.startSec = startSec;

		const result = await addClip(trackId, opts.filePath, startSec, clipEnd);
		const engineClip = clipFromResult(result);
		const engineClipId = engineClip?.id ?? null;
		if (engineClipId) {
			entry.engineClipId = engineClipId;
		}

		if (entry.cancelled) {
			await preparePromise.catch(() => {});
			await purgeImportEngineClip(trackId, opts.filePath, startSec, engineClipId ?? undefined);
			return;
		}

		if (engineClipId) {
			pendingImports.attachEngineClip(pendingId, engineClipId);
		}
		// Cached re-import may miss bulk peak IPC — seed from ADD_CLIP DTO.
		if (engineClip) {
			pendingImports.seedPeaks(pendingId, {
				sampleRate: engineClip.sample_rate,
				peaks_64: engineClip.peaks_64,
				peaks_512: engineClip.peaks_512,
				peaks_4096: engineClip.peaks_4096,
			});
		}

		const prepared = await preparePromise;
		if (entry.cancelled) {
			await purgeImportEngineClip(trackId, opts.filePath, startSec, entry.engineClipId);
			return;
		}

		if (prepared?.duration_sec > 0) {
			const latest = pendingImports.get(pendingId);
			const s = latest?.start ?? startSec;
			pendingImports.updatePlacement(pendingId, {
				start: s,
				end: s + prepared.duration_sec,
			});
		}

		if (entry.cancelled) {
			await purgeImportEngineClip(trackId, opts.filePath, startSec, entry.engineClipId);
			return;
		}

		pendingImports.markSettled(pendingId);
		if (entry.cancelled) {
			await purgeImportEngineClip(trackId, opts.filePath, startSec, entry.engineClipId);
			return;
		}
		notify(`Importé : ${name}`, 'success');
	} catch (e) {
		if (entry.cancelled) {
			await purgeImportEngineClip(
				entry.trackId,
				opts.filePath,
				entry.startSec,
				entry.engineClipId,
			);
			return;
		}
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
