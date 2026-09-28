import { writable } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface AudioImportProgress {
	/** Nested imports currently in flight. */
	activeCount: number;
	/** Display name of the most recent import. */
	fileName: string | null;
	/** Absolute path of the file being imported, when known. */
	filePath: string | null;
	/** 0..1 when known; null = indeterminate. */
	progress: number | null;
}

function createAudioImportProgress() {
	const { subscribe, update, set } = writable<AudioImportProgress>({
		activeCount: 0,
		fileName: null,
		filePath: null,
		progress: null,
	});

	let unlisten: UnlistenFn | null = null;

	return {
		subscribe,

		async connect() {
			if (unlisten) return;
			unlisten = await listen<{ file_path: string; progress: number; cached?: boolean }>(
				'audio://import-progress',
				({ payload }) => {
					update((s) => {
						if (s.activeCount <= 0) return s;
						if (s.filePath && payload.file_path) {
							const norm = (p: string) => p.replace(/\\/g, '/').toLowerCase();
							if (norm(s.filePath) !== norm(payload.file_path)) return s;
						}
						return {
							...s,
							filePath: s.filePath ?? payload.file_path,
							progress: Math.max(0, Math.min(1, payload.progress)),
						};
					});
				},
			);
		},

		disconnect() {
			unlisten?.();
			unlisten = null;
		},

		begin(fileName: string, filePath?: string) {
			update((s) => ({
				activeCount: s.activeCount + 1,
				fileName,
				filePath: filePath ?? s.filePath,
				progress: s.activeCount === 0 ? 0 : s.progress,
			}));
		},

		end() {
			update((s) => {
				const activeCount = Math.max(0, s.activeCount - 1);
				if (activeCount === 0) {
					return {
						activeCount: 0,
						fileName: null,
						filePath: null,
						progress: null,
					};
				}
				return {
					...s,
					activeCount,
				};
			});
		},

		reset() {
			set({
				activeCount: 0,
				fileName: null,
				filePath: null,
				progress: null,
			});
		},
	};
}

export const audioImportProgress = createAudioImportProgress();
