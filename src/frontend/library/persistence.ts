import { collectionIdOfSource, normalizeCollectionName } from './collections';
import { normalizeTag } from './tags';
import {
	isLibraryBuiltInSourceId,
	isLibraryKindFilter,
	type LibraryCollection,
	type LibrarySourceId
} from './types';

const STORAGE_KEY = 'echoes.library.v1';
const MAX_RECENTS = 50;

export interface LibraryPersistence {
	favoriteIds: string[];
	recentIds: string[];
	indexedFolders: string[];
	lastSource: LibrarySourceId;
	/** Tags the user gave to an asset, by asset id. */
	userTags: Record<string, string[]>;
	collections: LibraryCollection[];
}

function defaultPersistence(): LibraryPersistence {
	return {
		favoriteIds: [],
		recentIds: [],
		indexedFolders: [],
		lastSource: 'audio',
		userTags: {},
		collections: []
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

function collectionsFrom(value: unknown): LibraryCollection[] {
	if (!Array.isArray(value)) return [];
	const collections: LibraryCollection[] = [];
	const seenIds = new Set<string>();
	for (const raw of value) {
		if (typeof raw !== 'object' || raw === null) continue;
		const { id, name, query, kind } = raw as Record<string, unknown>;
		if (typeof id !== 'string' || !id || seenIds.has(id)) continue;
		if (typeof name !== 'string' || typeof query !== 'string') continue;
		const normalizedName = normalizeCollectionName(name);
		const normalizedKind = isLibraryKindFilter(kind) ? kind : null;
		if (!normalizedName || (!query.trim() && !normalizedKind)) continue;
		seenIds.add(id);
		collections.push({ id, name: normalizedName, query: query.trim(), kind: normalizedKind });
	}
	return collections;
}

function lastSourceFrom(
	value: unknown,
	collections: readonly LibraryCollection[]
): LibrarySourceId {
	if (typeof value !== 'string') return 'audio';
	if (isLibraryBuiltInSourceId(value)) return value;
	const collectionId = collectionIdOfSource(value as LibrarySourceId);
	return collectionId && collections.some((collection) => collection.id === collectionId)
		? (value as LibrarySourceId)
		: 'audio';
}

export function loadLibraryPersistence(): LibraryPersistence {
	if (!canUseStorage()) return defaultPersistence();
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		if (!raw) return defaultPersistence();
		const parsed = JSON.parse(raw) as Partial<Record<keyof LibraryPersistence, unknown>>;
		const collections = collectionsFrom(parsed.collections);
		return {
			favoriteIds: stringList(parsed.favoriteIds),
			recentIds: stringList(parsed.recentIds),
			indexedFolders: stringList(parsed.indexedFolders),
			lastSource: lastSourceFrom(parsed.lastSource, collections),
			userTags: userTagsFrom(parsed.userTags),
			collections
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
