import { isLibrarySourceId, type LibrarySourceId } from './types';

const STORAGE_KEY = 'echoes.library.v1';
const MAX_RECENTS = 50;

export interface LibraryPersistence {
	favoriteIds: string[];
	recentIds: string[];
	indexedFolders: string[];
	lastSource: LibrarySourceId;
}

const DEFAULTS: LibraryPersistence = {
	favoriteIds: [],
	recentIds: [],
	indexedFolders: [],
	lastSource: 'audio'
};

function canUseStorage(): boolean {
	return typeof localStorage !== 'undefined';
}

export function loadLibraryPersistence(): LibraryPersistence {
	if (!canUseStorage()) return { ...DEFAULTS, favoriteIds: [], recentIds: [], indexedFolders: [] };
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		if (!raw) return { ...DEFAULTS, favoriteIds: [], recentIds: [], indexedFolders: [] };
		const parsed = JSON.parse(raw) as Partial<LibraryPersistence>;
		const lastSource =
			typeof parsed.lastSource === 'string' && isLibrarySourceId(parsed.lastSource)
				? parsed.lastSource
				: 'audio';
		return {
			favoriteIds: Array.isArray(parsed.favoriteIds)
				? parsed.favoriteIds.filter((x) => typeof x === 'string')
				: [],
			recentIds: Array.isArray(parsed.recentIds)
				? parsed.recentIds.filter((x) => typeof x === 'string')
				: [],
			indexedFolders: Array.isArray(parsed.indexedFolders)
				? parsed.indexedFolders.filter((x) => typeof x === 'string')
				: [],
			lastSource
		};
	} catch {
		return { ...DEFAULTS, favoriteIds: [], recentIds: [], indexedFolders: [] };
	}
}

export function saveLibraryPersistence(data: LibraryPersistence): void {
	if (!canUseStorage()) return;
	try {
		localStorage.setItem(STORAGE_KEY, JSON.stringify(data));
	} catch {
		/* quota / private mode — ignore */
	}
}

export function pushRecentId(recentIds: string[], id: string): string[] {
	const next = [id, ...recentIds.filter((x) => x !== id)];
	return next.slice(0, MAX_RECENTS);
}
