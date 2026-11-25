import { derived, get, writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import {
	loadLibraryPersistence,
	pushRecentId,
	saveLibraryPersistence,
	type LibraryPersistence
} from './persistence';
import { filterAssetsByQuery } from './search';
import { mergeTags, withoutTag, withTag } from './tags';
import { libraryPreviewPlayer } from './previewPlayer';
import { libraryMidiPreviewPlayer } from './midiPreview';
import { FACTORY_LIBRARY_ASSETS, isFactoryAssetId } from './factoryCatalog';
import { drumIdsWithSoundsFirst, loadFactoryDrumAssets } from './factoryDrums';
import { resolveLibraryMidiNotes } from './resolveMidi';
import { isMidiLibraryKind } from './types';
import { projectEventStore } from '../../shared/project/eventStore';
import { tempo } from '../../stores/tempo';
import { notify } from '../../stores/status';
import {
	assetFromPath,
	assetFromScanEntry,
	type LibraryAsset,
	type LibraryIndexerState,
	type LibraryScanEntry,
	type LibrarySourceId
} from './types';

export interface LibraryState {
	source: LibrarySourceId;
	query: string;
	selectedId: string | null;
	previewingId: string | null;
	assetsById: Record<string, LibraryAsset>;
	audioIds: string[];
	chordIds: string[];
	midiIds: string[];
	drumIds: string[];
	favoriteIds: string[];
	recentIds: string[];
	indexedFolders: string[];
	userTags: Record<string, string[]>;
	indexer: LibraryIndexerState;
	indexerProgress: number | null;
	indexerError: string | null;
	scanToken: number;
}

function seedFactoryAssets(): {
	assetsById: Record<string, LibraryAsset>;
	chordIds: string[];
	midiIds: string[];
	drumIds: string[];
} {
	const assetsById: Record<string, LibraryAsset> = {};
	const chordIds: string[] = [];
	const midiIds: string[] = [];
	const drumIds: string[] = [];
	for (const asset of FACTORY_LIBRARY_ASSETS) {
		assetsById[asset.id] = asset;
		if (asset.kind === 'chord_progression') chordIds.push(asset.id);
		else if (asset.tags?.includes('drums')) drumIds.push(asset.id);
		else if (asset.kind === 'midi_pattern') midiIds.push(asset.id);
	}
	return { assetsById, chordIds, midiIds, drumIds };
}

function initialState(): LibraryState {
	const persisted = loadLibraryPersistence();
	const factory = seedFactoryAssets();
	return {
		source: persisted.lastSource,
		query: '',
		selectedId: null,
		previewingId: null,
		assetsById: factory.assetsById,
		audioIds: [],
		chordIds: factory.chordIds,
		midiIds: factory.midiIds,
		drumIds: factory.drumIds,
		favoriteIds: [...persisted.favoriteIds],
		recentIds: [...persisted.recentIds],
		indexedFolders: [...persisted.indexedFolders],
		userTags: persisted.userTags,
		indexer: 'idle',
		indexerProgress: null,
		indexerError: null,
		scanToken: 0
	};
}

function persistSlice(state: LibraryState): void {
	const data: LibraryPersistence = {
		favoriteIds: state.favoriteIds,
		recentIds: state.recentIds,
		indexedFolders: state.indexedFolders,
		lastSource: state.source,
		userTags: state.userTags
	};
	saveLibraryPersistence(data);
}

function mergedAsset(previous: LibraryAsset | undefined, asset: LibraryAsset): LibraryAsset {
	if (!previous) return asset;
	return {
		...previous,
		...asset,
		durationSec: asset.durationSec ?? previous.durationSec,
		lengthBeats: asset.lengthBeats ?? previous.lengthBeats,
		notes: asset.notes ?? previous.notes,
		chords: asset.chords ?? previous.chords,
		keyRoot: asset.keyRoot ?? previous.keyRoot,
		tags: asset.tags ?? previous.tags,
		origin: asset.origin ?? previous.origin
	};
}

function mergeAsset(
	map: Record<string, LibraryAsset>,
	asset: LibraryAsset
): Record<string, LibraryAsset> {
	return { ...map, [asset.id]: mergedAsset(map[asset.id], asset) };
}

function isInsideFolder(path: string, folderPath: string): boolean {
	const normalize = (p: string) => p.replace(/\\/g, '/').toLowerCase();
	const base = normalize(folderPath).replace(/\/+$/, '');
	return normalize(path).startsWith(`${base}/`);
}

function createLibraryStore() {
	const { subscribe, update, set } = writable<LibraryState>(initialState());

	const activeScans = new Map<number, { folderPath: string; seenIds: Set<string> }>();
	let scanUnlisteners: UnlistenFn[] = [];
	let hoverTimer: ReturnType<typeof setTimeout> | null = null;
	let hydrated = false;

	function clearHoverTimer() {
		if (hoverTimer != null) {
			clearTimeout(hoverTimer);
			hoverTimer = null;
		}
	}

	function stopAllPreviewPlayers() {
		libraryPreviewPlayer.stop();
		libraryMidiPreviewPlayer.stop();
	}

	libraryPreviewPlayer.setOnEnded(() => {
		update((s) => (s.previewingId == null ? s : { ...s, previewingId: null }));
	});
	libraryMidiPreviewPlayer.setOnEnded(() => {
		update((s) => (s.previewingId == null ? s : { ...s, previewingId: null }));
	});

	function failScan(message: string) {
		update((s) => ({
			...s,
			indexer: 'error',
			indexerError: message,
			indexerProgress: null
		}));
	}

	async function startScan(folderPath: string) {
		await ensureScanListeners();
		const token = get({ subscribe }).scanToken + 1;
		activeScans.set(token, { folderPath, seenIds: new Set() });
		update((s) => ({
			...s,
			scanToken: token,
			indexer: 'scanning',
			indexerProgress: 0,
			indexerError: null
		}));
		try {
			await invoke('scan_library_folder', { folderPath, token });
		} catch (e) {
			activeScans.delete(token);
			failScan(e instanceof Error ? e.message : String(e));
		}
	}

	async function ensureScanListeners() {
		if (scanUnlisteners.length > 0) return;
		scanUnlisteners = await Promise.all([
			listen<{ token: number; entries: LibraryScanEntry[]; progress: number }>(
				'library://scan-batch',
				({ payload }) => {
					const scan = activeScans.get(payload.token);
					if (!scan) return;
					update((s) => {
						const assetsById = { ...s.assetsById };
						const knownIds = new Set(s.audioIds);
						const newIds: string[] = [];
						for (const entry of payload.entries) {
							const asset = assetFromScanEntry(entry);
							scan.seenIds.add(asset.id);
							assetsById[asset.id] = mergedAsset(assetsById[asset.id], asset);
							if (!knownIds.has(asset.id)) {
								knownIds.add(asset.id);
								newIds.push(asset.id);
							}
						}
						return {
							...s,
							assetsById,
							audioIds: [...s.audioIds, ...newIds],
							indexer: 'scanning',
							indexerProgress: payload.progress,
							indexerError: null
						};
					});
				}
			),
			listen<{ token: number; total: number }>('library://scan-done', ({ payload }) => {
				const scan = activeScans.get(payload.token);
				if (!scan) return;
				activeScans.delete(payload.token);
				update((s) => {
					const missingIds = new Set(
						s.audioIds.filter((id) => isInsideFolder(id, scan.folderPath) && !scan.seenIds.has(id))
					);
					const assetsById = { ...s.assetsById };
					for (const id of missingIds) delete assetsById[id];
					return {
						...s,
						assetsById,
						audioIds:
							missingIds.size > 0 ? s.audioIds.filter((id) => !missingIds.has(id)) : s.audioIds,
						indexer: activeScans.size > 0 ? 'scanning' : 'ready',
						indexerProgress: 1,
						indexerError: null
					};
				});
			}),
			listen<{ token: number; message: string }>('library://scan-error', ({ payload }) => {
				if (!activeScans.delete(payload.token)) return;
				failScan(payload.message);
			})
		]);
	}

	const api = {
		subscribe,

		hydrateFromPersistence() {
			if (hydrated) return;
			hydrated = true;
			set(initialState());
			void api.loadFactoryDrums();
		},

		async loadFactoryDrums() {
			try {
				const sounds = await loadFactoryDrumAssets();
				update((s) => ({
					...s,
					assetsById: sounds.reduce(mergeAsset, s.assetsById),
					drumIds: drumIdsWithSoundsFirst(
						s.drumIds,
						sounds.map((sound) => sound.id)
					)
				}));
			} catch (e) {
				notify(`Sons de batterie indisponibles : ${(e as Error)?.message ?? String(e)}`, 'error');
			}
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

		addTag(id: string, rawTag: string) {
			update((s) => {
				const current = s.userTags[id] ?? [];
				const tags = withTag(current, rawTag);
				if (tags === current) return s;
				const next = { ...s, userTags: { ...s.userTags, [id]: [...tags] } };
				persistSlice(next);
				return next;
			});
		},

		removeTag(id: string, tag: string) {
			update((s) => {
				const current = s.userTags[id];
				if (!current?.includes(tag)) return s;
				const remaining = withoutTag(current, tag);
				const userTags = { ...s.userTags, [id]: remaining };
				if (remaining.length === 0) delete userTags[id];
				const next = { ...s, userTags };
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
			update((s) => {
				if (s.indexedFolders.includes(folderPath)) return s;
				const next = { ...s, indexedFolders: [...s.indexedFolders, folderPath] };
				persistSlice(next);
				return next;
			});
			await startScan(folderPath);
		},

		async rescanIndexedFolders() {
			for (const folderPath of get({ subscribe }).indexedFolders) {
				await startScan(folderPath);
			}
		},

		async playPreview(id: string) {
			const state = get({ subscribe });
			const asset = state.assetsById[id] ?? assetFromPath(id);
			clearHoverTimer();

			if (isMidiLibraryKind(asset.kind)) {
				if (libraryMidiPreviewPlayer.isPlaying(id)) {
					update((s) => ({ ...s, selectedId: id, previewingId: id }));
					return;
				}
				stopAllPreviewPlayers();
				const projectKey = get(projectEventStore).key_root ?? 'C';
				const notes = resolveLibraryMidiNotes(asset, {
					projectKeyRoot: projectKey,
					transposeForPreview: true
				});
				if (notes.length === 0) return;
				try {
					update((s) => ({ ...s, selectedId: id, previewingId: id }));
					await libraryMidiPreviewPlayer.play(id, notes, get(tempo).bpm);
					api.recordRecent(id);
				} catch (e) {
					libraryMidiPreviewPlayer.stop();
					update((s) => ({ ...s, previewingId: null }));
					throw e;
				}
				return;
			}

			if (libraryPreviewPlayer.isPlaying(asset.path)) {
				update((s) => ({ ...s, selectedId: id, previewingId: id }));
				return;
			}
			try {
				stopAllPreviewPlayers();
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
				if (isMidiLibraryKind(asset.kind)) {
					if (libraryMidiPreviewPlayer.isPlaying(id)) {
						libraryMidiPreviewPlayer.stop();
						update((s) => ({ ...s, previewingId: null }));
						return;
					}
				} else if (libraryPreviewPlayer.isPlaying(asset.path)) {
					libraryPreviewPlayer.stop();
					update((s) => ({ ...s, previewingId: null }));
					return;
				}
				await api.playPreview(id);
			} catch (e) {
				stopAllPreviewPlayers();
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
			stopAllPreviewPlayers();
			update((s) => ({ ...s, previewingId: null }));
		},

		getSelectedAsset(): LibraryAsset | null {
			const s = get({ subscribe });
			if (!s.selectedId) return null;
			return s.assetsById[s.selectedId] ?? assetFromPath(s.selectedId);
		},

		getAsset(id: string): LibraryAsset | null {
			const s = get({ subscribe });
			return s.assetsById[id] ?? null;
		}
	};

	return api;
}

export const libraryStore = createLibraryStore();

function withUserTags(asset: LibraryAsset, userTags: Record<string, string[]>): LibraryAsset {
	const own = userTags[asset.id];
	return own?.length ? { ...asset, tags: mergeTags(asset.tags, own) } : asset;
}

function idsForSource(state: LibraryState): string[] {
	switch (state.source) {
		case 'recents':
			return state.recentIds;
		case 'favorites':
			return state.favoriteIds;
		case 'chords':
			return state.chordIds;
		case 'midi':
			return state.midiIds;
		case 'drums':
			return state.drumIds;
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
			assets.push(withUserTags(asset, $lib.userTags));
			continue;
		}
		if (($lib.source === 'favorites' || $lib.source === 'recents') && !isFactoryAssetId(id)) {
			assets.push(withUserTags(assetFromPath(id), $lib.userTags));
		}
	}
	return filterAssetsByQuery(assets, $lib.query);
});

export const libraryHasIndexedContent = derived(libraryStore, ($lib) => {
	return (
		$lib.audioIds.length > 0 ||
		$lib.indexedFolders.length > 0 ||
		$lib.chordIds.length > 0 ||
		$lib.midiIds.length > 0 ||
		$lib.drumIds.length > 0
	);
});
