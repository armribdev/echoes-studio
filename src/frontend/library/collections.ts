import { filterAssetsByKind, filterAssetsByQuery } from './search';
import type {
	LibraryAsset,
	LibraryCollection,
	LibraryCollectionSourceId,
	LibraryKindFilter,
	LibrarySourceId
} from './types';

export const MAX_COLLECTION_NAME_LENGTH = 32;

const SOURCE_PREFIX = 'collection:';

export function collectionSourceId(collectionId: string): LibraryCollectionSourceId {
	return `${SOURCE_PREFIX}${collectionId}`;
}

export function collectionIdOfSource(source: LibrarySourceId): string | null {
	return source.startsWith(SOURCE_PREFIX) ? source.slice(SOURCE_PREFIX.length) : null;
}

export function collectionForSource(
	collections: readonly LibraryCollection[],
	source: LibrarySourceId
): LibraryCollection | null {
	const id = collectionIdOfSource(source);
	return id ? (collections.find((collection) => collection.id === id) ?? null) : null;
}

export function normalizeCollectionName(raw: string): string {
	return raw.trim().replace(/\s+/g, ' ').slice(0, MAX_COLLECTION_NAME_LENGTH).trim();
}

/** A collection needs at least one criterion, otherwise it would list everything. */
export function canSaveAsCollection(query: string, kind: LibraryKindFilter | null): boolean {
	return query.trim() !== '' || kind !== null;
}

export function createCollection(
	name: string,
	query: string,
	kind: LibraryKindFilter | null
): LibraryCollection {
	return {
		id: crypto.randomUUID(),
		name: normalizeCollectionName(name),
		query: query.trim(),
		kind
	};
}

export function filterAssets(
	assets: LibraryAsset[],
	filter: { query: string; kind: LibraryKindFilter | null }
): LibraryAsset[] {
	return filterAssetsByQuery(filterAssetsByKind(assets, filter.kind), filter.query);
}
