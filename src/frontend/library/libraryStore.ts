import { derived, get, writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import {
	loadLibraryPersistence,
	pushRecentId,
	saveLibraryPersistence,
	type LibraryPersistence,
} from './persistence';
import { filterAssetsByQuery } from './search';
import { libraryPreviewPlayer } from './previewPlayer';
import {
	assetFromPath,
	assetFromScanEntry,
	type LibraryAsset,
	type LibraryIndexerState,
	type LibraryScanEntry,
	type LibrarySourceId,
} from './types';

export interface LibraryState {
	source: LibrarySourceId;
	query: string;
	selectedId: string | null;
	previewingId: string | null;
	assetsById: Record<string, LibraryAsset>;
	audioIds: string[];
	favoriteIds: string[];
	recentIds: string[];
	indexedFolders: string[];
	indexer: LibraryIndexerState;
	indexerProgress: number | null;
	indexerError: string | null;
	scanToken: number;
}

function initialState(): LibraryState {
	const persisted = loadLibraryPersistence();
	return {
		source: persisted.lastSource,
		query: '',
		selectedId: null,
		previewingId: null,
		assetsById: {},
		audioIds: [],
		favoriteIds: [...persisted.favoriteIds],
		recentIds: [...persisted.recentIds],
		indexedFolders: [...persisted.indexedFolders],
		indexer: persisted.indexedFolders.length > 0 ? 'idle' : 'idle',
		indexerProgress: null,
		indexerError: null,
		scanToken: 0,
	};
}

function persistSlice(state: LibraryState): void {
	const data: LibraryPersistence = {
		favoriteIds: state.favoriteIds,
		recentIds: state.recentIds,
		indexedFolders: state.indexedFolders,
		lastSource: state.source,
	};
	saveLibraryPersistence(data);
}

function mergeAsset(map: Record<string, LibraryAsset>, asset: LibraryAsset): Record<string, LibraryAsset> {
	const prev = map[asset.id];
	if (!prev) return { ...map, [asset.id]: asset };
	return {
		...map,
		[asset.id]: {
			...prev,
			...asset,
			durationSec: asset.durationSec ?? prev.durationSec,
		},
	};
}

function createLibraryStore() {
	const { subscribe, update, set } = writable<LibraryState>(initialState());

	let scanUnlisteners: UnlistenFn[] = [];
	let hoverTimer: ReturnType<typeof setTimeout> | null = null;
	let hydrated = false;

	function clearHoverTimer() {
		if (hoverTimer != null) {
			clearTimeout(hoverTimer);
			hoverTimer = null;
		}
	}

	libraryPreviewPlayer.setOnEnded(() => {
		update((s) => (s.previewingId == null ? s : { ...s, previewingId: null }));
	});

	async function ensureScanListeners() {
		if (scanUnlisteners.length > 0) return;
		scanUnlisteners = await Promise.all([
			listen<{ token: number; entries: LibraryScanEntry[]; progress: number }>(
				'library://scan-batch',
				({ payload }) => {
					update((s) => {
						if (payload.token !== s.scanToken) return s;
						let assetsById = s.assetsById;
						const audioIds = [...s.audioIds];
						const seen = new Set(audioIds);
						for (const entry of payload.entries) {
							const asset = assetFromScanEntry(entry);
							assetsById = mergeAsset(assetsById, asset);
							if (!seen.has(asset.id)) {
								seen.add(asset.id);
								audioIds.push(asset.id);
							}
						}
						return {
							...s,
							assetsById,
							audioIds,
							indexer: 'scanning',
							indexerProgress: payload.progress,
							indexerError: null,
						};
					});
				},
			),
			listen<{ token: number; total: number }>('library://scan-done', ({ payload }) => {
				update((s) => {
					if (payload.token !== s.scanToken) return s;
					return {
						...s,
						indexer: 'ready',
						indexerProgress: 1,
						indexerError: null,
					};
				});
			}),
			listen<{ token: number; message: string }>('library://scan-error', ({ payload }) => {
				update((s) => {
					if (payload.token !== s.scanToken) return s;
					return {
						...s,
						indexer: 'error',
						indexerError: payload.message,
						indexerProgress: null,
					};
				});
			}),
		]);
	}

	const api = {
		subscribe,

		hydrateFromPersistence() {
			if (hydrated) return;
			hydrated = true;
			set(initialState());
		},

		setSource(source: LibrarySourceId) {
			update((s) => {
				const next = { ...s, source, selectedId: null };
				persistSlice(next);
				return next;
			});
		},

		setQuery(query: string) {
			update((s) => ({ ...s, query }));
		},

		clearQuery() {
			update((s) => ({ ...s, query: '' }));
		},

		select(id: string | null) {
			update((s) => ({ ...s, selectedId: id }));
		},

		moveSelection(delta: number, visibleIds: string[]) {
			if (visibleIds.length === 0) return;
			update((s) => {
				const current = s.selectedId ? visibleIds.indexOf(s.selectedId) : -1;
				let nextIndex: number;
				if (current < 0) {
					nextIndex = delta > 0 ? 0 : visibleIds.length - 1;
				} else {
					nextIndex = Math.max(0, Math.min(visibleIds.length - 1, current + delta));
				}
				return { ...s, selectedId: visibleIds[nextIndex] ?? null };
			});
		},

		toggleFavorite(id: string) {
			update((s) => {
				const favoriteIds = s.favoriteIds.includes(id)
					? s.favoriteIds.filter((x) => x !== id)
					: [...s.favoriteIds, id];
				const next = { ...s, favoriteIds };
				persistSlice(next);
				return next;
			});
		},

		recordRecent(id: string) {
			update((s) => {
				const recentIds = pushRecentId(s.recentIds, id);
				const next = { ...s, recentIds };
				persistSlice(next);
				return next;
			});
		},

		upsertAsset(asset: LibraryAsset) {
			update((s) => {
				const assetsById = mergeAsset(s.assetsById, asset);
				const audioIds = s.audioIds.includes(asset.id) ? s.audioIds : [...s.audioIds, asset.id];
				return { ...s, assetsById, audioIds };
			});
		},

		importFiles(paths: string[]) {
			update((s) => {
				let assetsById = s.assetsById;
				const audioIds = [...s.audioIds];
				const seen = new Set(audioIds);
				for (const path of paths) {
					const asset = assetFromPath(path);
					assetsById = mergeAsset(assetsById, asset);
					if (!seen.has(asset.id)) {
						seen.add(asset.id);
						audioIds.push(asset.id);
					}
				}
				return { ...s, assetsById, audioIds };
			});
		},

		async addFolder(folderPath: string) {
			await ensureScanListeners();
			update((s) => {
				const indexedFolders = s.indexedFolders.includes(folderPath)
					? s.indexedFolders
					: [...s.indexedFolders, folderPath];
				const scanToken = s.scanToken + 1;
				const next = {
					...s,
					indexedFolders,
					scanToken,
					indexer: 'scanning' as const,
					indexerProgress: 0,
					indexerError: null,
				};
				persistSlice(next);
				return next;
			});
			const token = get({ subscribe }).scanToken;
			try {
				await invoke('scan_library_folder', { folderPath, token });
			} catch (e) {
				update((s) => ({
					...s,
					indexer: 'error',
					indexerError: e instanceof Error ? e.message : String(e),
					indexerProgress: null,
				}));
			}
		},

		async rescanIndexedFolders() {
			const folders = [...get({ subscribe }).indexedFolders];
			for (const folderPath of folders) {
				await ensureScanListeners();
				update((s) => {
					const scanToken = s.scanToken + 1;
					return {
						...s,
						scanToken,
						indexer: 'scanning',
						indexerProgress: 0,
						indexerError: null,
					};
				});
				const token = get({ subscribe }).scanToken;
				try {
					await invoke('scan_library_folder', { folderPath, token });
				} catch (e) {
					update((s) => ({
						...s,
						indexer: 'error',
						indexerError: e instanceof Error ? e.message : String(e),
						indexerProgress: null,
					}));
					return;
				}
			}
		},

		async playPreview(id: string) {
			const state = get({ subscribe });
			const asset = state.assetsById[id] ?? assetFromPath(id);
			clearHoverTimer();
			if (libraryPreviewPlayer.isPlaying(asset.path)) {
				update((s) => ({ ...s, selectedId: id, previewingId: id }));
				return;
			}
			try {
				update((s) => ({ ...s, selectedId: id, previewingId: id }));
				await libraryPreviewPlayer.play(asset.path);
				api.recordRecent(id);
			} catch (e) {
				libraryPreviewPlayer.stop();
				update((s) => ({ ...s, previewingId: null }));
				throw e;
			}
		},

		async togglePreview(id: string) {
			const state = get({ subscribe });
			const asset = state.assetsById[id] ?? assetFromPath(id);
			clearHoverTimer();
			try {
				if (libraryPreviewPlayer.isPlaying(asset.path)) {
					libraryPreviewPlayer.stop();
					update((s) => ({ ...s, previewingId: null }));
					return;
				}
				await api.playPreview(id);
			} catch (e) {
				libraryPreviewPlayer.stop();
				update((s) => ({ ...s, previewingId: null }));
				throw e;
			}
		},

		scheduleHoverPreview(id: string) {
			clearHoverTimer();
			hoverTimer = setTimeout(() => {
				hoverTimer = null;
				void api.playPreview(id).catch(() => {});
			}, 300);
		},

		cancelHoverPreview() {
			clearHoverTimer();
		},

		stopPreview() {
			clearHoverTimer();
			libraryPreviewPlayer.stop();
			update((s) => ({ ...s, previewingId: null }));
		},

		getSelectedAsset(): LibraryAsset | null {
			const s = get({ subscribe });
			if (!s.selectedId) return null;
			return s.assetsById[s.selectedId] ?? assetFromPath(s.selectedId);
		},
	};

	return api;
}

export const libraryStore = createLibraryStore();

function idsForSource(state: LibraryState): string[] {
	switch (state.source) {
		case 'recents':
			return state.recentIds;
		case 'favorites':
			return state.favoriteIds;
		case 'audio':
		default:
			return state.audioIds;
	}
}

export const libraryVisibleAssets = derived(libraryStore, ($lib) => {
	const ids = idsForSource($lib);
	const assets: LibraryAsset[] = [];
	for (const id of ids) {
		const asset = $lib.assetsById[id];
		if (asset) {
			assets.push(asset);
			continue;
		}
		// Favorites/recents may reference paths before rescan — synthesise a stub.
		if ($lib.source === 'favorites' || $lib.source === 'recents') {
			assets.push(assetFromPath(id));
		}
	}
	return filterAssetsByQuery(assets, $lib.query);
});

export const libraryHasIndexedContent = derived(libraryStore, ($lib) => {
	return $lib.audioIds.length > 0 || $lib.indexedFolders.length > 0;
});
