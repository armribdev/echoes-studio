import { get, writable } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

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

						// Full replace (finalize / cache hit) wins over deltas.
						if (payload.peaks_64 && payload.peaks_64.length > 0) {
							next.peaks_64 = payload.peaks_64;
						}
						if (payload.peaks_512 && payload.peaks_512.length > 0) {
							next.peaks_512 = payload.peaks_512;
						}
						if (payload.peaks_4096 && payload.peaks_4096.length > 0) {
							next.peaks_4096 = payload.peaks_4096;
						}

						if (
							payload.decoded_duration_sec != null &&
							payload.decoded_duration_sec > 0
						) {
							next.end = Math.max(
								item.end,
								item.start + payload.decoded_duration_sec,
							);
						}
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

		remove(id: string) {
			update((list) => list.filter((item) => item.id !== id));
		},

		removeByPath(filePath: string) {
			update((list) => list.filter((item) => item.filePath !== filePath));
		},

		clear() {
			set([]);
		},

		has(id: string): boolean {
			return get({ subscribe }).some((item) => item.id === id);
		},
	};
}

export const pendingImports = createPendingImports();
