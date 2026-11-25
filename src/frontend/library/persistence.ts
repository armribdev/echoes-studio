import { normalizeCollectionName } from './collections';
import { hasSourceShape, isKnownSource, type KnownSources } from './sources';
import { normalizeTag } from './tags';
import {
	isLibraryKindFilter,
	isLibraryOpenContext,
	type LibraryCollection,
	type LibraryContextSources,
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
	/** Sources the user chose to open each context on. */
	contextSources: LibraryContextSources;
}

function defaultPersistence(): LibraryPersistence {
	return {
		favoriteIds: [],
		recentIds: [],
		indexedFolders: [],
		lastSource: 'audio',
		userTags: {},
		collections: [],
		contextSources: {}
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

function lastSourceFrom(value: unknown, known: KnownSources): LibrarySourceId {
	return typeof value === 'string' && hasSourceShape(value) && isKnownSource(value, known)
		? value
		: 'audio';
}

function contextSourcesFrom(value: unknown): LibraryContextSources {
	const contextSources: LibraryContextSources = {};
	if (typeof value !== 'object' || value === null) return contextSources;
	for (const [context, source] of Object.entries(value)) {
		if (!isLibraryOpenContext(context) || typeof source !== 'string') continue;
		if (hasSourceShape(source)) contextSources[context] = source;
	}
	return contextSources;
}

export function loadLibraryPersistence(): LibraryPersistence {
	if (!canUseStorage()) return defaultPersistence();
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		if (!raw) return defaultPersistence();
		const parsed = JSON.parse(raw) as Partial<Record<keyof LibraryPersistence, unknown>>;
		const collections = collectionsFrom(parsed.collections);
		const indexedFolders = stringList(parsed.indexedFolders);
		return {
			favoriteIds: stringList(parsed.favoriteIds),
			recentIds: stringList(parsed.recentIds),
			indexedFolders,
			lastSource: lastSourceFrom(parsed.lastSource, { collections, indexedFolders }),
			userTags: userTagsFrom(parsed.userTags),
			collections,
			contextSources: contextSourcesFrom(parsed.contextSources)
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
