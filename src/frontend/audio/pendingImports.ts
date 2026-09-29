import { get, writable } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { forgetClipUiKey, rememberClipUiKey } from './clipDisplay';
import { selection } from '../../stores/selection';

export type PeakPair = [number, number];

export interface PendingImportClip {
	id: string;
	trackId: string;
	filePath: string;
	fileName: string;
	start: number;
	end: number;
	sampleRate: number;
	peaks_64: PeakPair[];
	peaks_512: PeakPair[];
	peaks_4096: PeakPair[];
	progress: number;
	/** Set after ADD_CLIP — same Clip instance flips pending→false. */
	engineClipId?: string;
	/**
	 * Import finished — keep this slot so the Clip keeps streamed peaks.
	 * Removing it was swapping in engine peaks and flashing at "Importé !".
	 */
	settled?: boolean;
}

interface ImportProgressPayload {
	file_path: string;
	progress: number;
	cached?: boolean;
	sample_rate?: number;
	peaks_64?: PeakPair[];
	peaks_512?: PeakPair[];
	peaks_4096?: PeakPair[];
	peaks_512_from?: number;
	peaks_512_append?: PeakPair[];
	peaks_4096_from?: number;
	peaks_4096_append?: PeakPair[];
	decoded_duration_sec?: number;
}

function pathsMatch(a: string, b: string): boolean {
	const norm = (p: string) => p.replace(/\\/g, '/').toLowerCase();
	return norm(a) === norm(b);
}

function applyPeakDelta(
	current: PeakPair[],
	from: number | undefined,
	append: PeakPair[] | undefined,
): PeakPair[] | null {
	if (from == null || !append || append.length === 0) return null;
	const head = from <= current.length ? current.slice(0, from) : current.slice();
	return head.concat(append);
}

function createPendingImports() {
	const { subscribe, update, set } = writable<PendingImportClip[]>([]);
	let unlisten: UnlistenFn | null = null;
	let seq = 0;
	const paintWaiters = new Map<string, Array<() => void>>();
	const dragLocked = new Set<string>();
	const removeWhenUnlocked = new Set<string>();

	function resolvePaint(id: string) {
		const waiters = paintWaiters.get(id);
		if (!waiters?.length) return;
		paintWaiters.delete(id);
		for (const w of waiters) w();
	}

	function removeNow(id: string) {
		update((list) => list.filter((x) => x.id !== id));
		resolvePaint(id);
		dragLocked.delete(id);
		removeWhenUnlocked.delete(id);
	}

	return {
		subscribe,

		async connect() {
			if (unlisten) return;
			unlisten = await listen<ImportProgressPayload>('audio://import-progress', ({ payload }) => {
				update((list) =>
					list.map((item) => {
						if (!pathsMatch(item.filePath, payload.file_path)) return item;
						const next: PendingImportClip = {
							...item,
							progress: Math.max(item.progress, payload.progress),
						};
						if (payload.sample_rate && payload.sample_rate > 0) {
							next.sampleRate = payload.sample_rate;
						}

						const d512 = applyPeakDelta(
							item.peaks_512,
							payload.peaks_512_from,
							payload.peaks_512_append,
						);
						if (d512) next.peaks_512 = d512;
						const d4096 = applyPeakDelta(
							item.peaks_4096,
							payload.peaks_4096_from,
							payload.peaks_4096_append,
						);
						if (d4096) next.peaks_4096 = d4096;

						if (payload.peaks_64 && payload.peaks_64.length > 0) {
							next.peaks_64 = payload.peaks_64;
						}
						// Skip finalize full-replace of streamed lanes — recomputed
						// peaks differ slightly and flash the waveform at the end.
						if (payload.peaks_512 && payload.peaks_512.length > 0 && item.peaks_512.length === 0) {
							next.peaks_512 = payload.peaks_512;
						}
						if (payload.peaks_4096 && payload.peaks_4096.length > 0 && item.peaks_4096.length === 0) {
							next.peaks_4096 = payload.peaks_4096;
						}

						// Do not resize the clip frame from progressive decode length —
						// that shrinks a probed full-duration slot then grows it back.
						// Frame size is set at add() / prepare finalize only.
						return next;
					}),
				);
			});
		},

		disconnect() {
			unlisten?.();
			unlisten = null;
		},

		add(input: {
			trackId: string;
			filePath: string;
			fileName: string;
			start: number;
			end: number;
			sampleRate?: number;
		}): string {
			const id = `pending-import-${++seq}-${Date.now()}`;
			update((list) => [
				...list,
				{
					id,
					trackId: input.trackId,
					filePath: input.filePath,
					fileName: input.fileName,
					start: input.start,
					end: Math.max(input.end, input.start + 0.25),
					sampleRate: input.sampleRate ?? 48_000,
					peaks_64: [],
					peaks_512: [],
					peaks_4096: [],
					progress: 0,
				},
			]);
			return id;
		},

		get(id: string): PendingImportClip | undefined {
			return get({ subscribe }).find((item) => item.id === id);
		},

		updatePlacement(
			id: string,
			placement: { start: number; end: number; trackId?: string },
		) {
			update((list) =>
				list.map((item) => {
					if (item.id !== id) return item;
					return {
						...item,
						start: placement.start,
						end: Math.max(placement.end, placement.start + 0.05),
						trackId: placement.trackId ?? item.trackId,
					};
				}),
			);
		},

		/** Bind engine clip while keeping the same UI key (= pending id). */
		attachEngineClip(id: string, engineClipId: string) {
			rememberClipUiKey(engineClipId, id);
			selection.remapId(id, engineClipId);
			update((list) =>
				list.map((item) =>
					item.id === id ? { ...item, engineClipId } : item,
				),
			);
		},

		/**
		 * Import committed — keep the slot (and streamed peaks) so we never
		 * destroy/replace the Clip waveform at the success notify.
		 */
		markSettled(id: string) {
			update((list) =>
				list.map((item) => (item.id === id ? { ...item, settled: true, progress: 1 } : item)),
			);
			resolvePaint(id);
		},

		/** Drop settled slots whose engine clip no longer exists on any track. */
		pruneOrphans(liveClipIds: Set<string>) {
			const doomed = get({ subscribe }).filter(
				(item) =>
					item.settled &&
					item.engineClipId != null &&
					!liveClipIds.has(item.engineClipId),
			);
			if (doomed.length === 0) return;
			for (const item of doomed) {
				if (item.engineClipId) forgetClipUiKey(item.engineClipId);
				removeNow(item.id);
			}
		},

		/** Clip finished first committed paint — safe to drop the pending slot. */
		signalPainted(pendingId: string) {
			resolvePaint(pendingId);
		},

		waitForPaint(pendingId: string, timeoutMs = 450): Promise<void> {
			return new Promise((resolve) => {
				let settled = false;
				const done = () => {
					if (settled) return;
					settled = true;
					resolve();
				};
				const list = paintWaiters.get(pendingId) ?? [];
				list.push(done);
				paintWaiters.set(pendingId, list);
				setTimeout(done, timeoutMs);
			});
		},

		/** Keep slot alive while the user is dragging it (avoids jump on handoff). */
		lockDrag(id: string) {
			dragLocked.add(id);
		},

		unlockDrag(id: string) {
			dragLocked.delete(id);
			if (removeWhenUnlocked.has(id)) {
				removeWhenUnlocked.delete(id);
				removeNow(id);
			}
		},

		remove(id: string) {
			if (dragLocked.has(id)) {
				removeWhenUnlocked.add(id);
				resolvePaint(id);
				return;
			}
			removeNow(id);
		},

		removeByPath(filePath: string) {
			const doomed = get({ subscribe }).filter((item) => pathsMatch(item.filePath, filePath));
			for (const item of doomed) {
				if (item.engineClipId) forgetClipUiKey(item.engineClipId);
				removeNow(item.id);
			}
		},

		clear() {
			const all = get({ subscribe });
			set([]);
			dragLocked.clear();
			removeWhenUnlocked.clear();
			for (const item of all) {
				resolvePaint(item.id);
				if (item.engineClipId) forgetClipUiKey(item.engineClipId);
			}
		},

		has(id: string): boolean {
			return get({ subscribe }).some((item) => item.id === id);
		},
	};
}

export const pendingImports = createPendingImports();
