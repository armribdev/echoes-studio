import { normalizeTag } from './tags';
import { isLibrarySourceId, type LibrarySourceId } from './types';

const STORAGE_KEY = 'echoes.library.v1';
const MAX_RECENTS = 50;

export interface LibraryPersistence {
	favoriteIds: string[];
	recentIds: string[];
	indexedFolders: string[];
	lastSource: LibrarySourceId;
	/** Tags the user gave to an asset, by asset id. */
	userTags: Record<string, string[]>;
}

function defaultPersistence(): LibraryPersistence {
	return {
		favoriteIds: [],
		recentIds: [],
		indexedFolders: [],
		lastSource: 'audio',
		userTags: {}
	};
}

function canUseStorage(): boolean {
	return typeof localStorage !== 'undefined';
}

function stringList(value: unknown): string[] {
	return Array.isArray(value) ? value.filter((x) => typeof x === 'string') : [];
}

function userTagsFrom(value: unknown): Record<string, string[]> {
	const userTags: Record<string, string[]> = {};
	if (typeof value !== 'object' || value === null) return userTags;
	for (const [id, rawTags] of Object.entries(value)) {
		const tags = [...new Set(stringList(rawTags).map(normalizeTag).filter(Boolean))];
		if (tags.length > 0) userTags[id] = tags;
	}
	return userTags;
}

export function loadLibraryPersistence(): LibraryPersistence {
	if (!canUseStorage()) return defaultPersistence();
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		if (!raw) return defaultPersistence();
		const parsed = JSON.parse(raw) as Partial<Record<keyof LibraryPersistence, unknown>>;
		return {
			favoriteIds: stringList(parsed.favoriteIds),
			recentIds: stringList(parsed.recentIds),
			indexedFolders: stringList(parsed.indexedFolders),
			lastSource:
				typeof parsed.lastSource === 'string' && isLibrarySourceId(parsed.lastSource)
					? parsed.lastSource
					: 'audio',
			userTags: userTagsFrom(parsed.userTags)
		};
	} catch {
		return defaultPersistence();
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
