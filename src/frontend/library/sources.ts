import { collectionIdOfSource } from './collections';
import { packFolderOfSource } from './packs';
import { isLibraryBuiltInSourceId, type LibraryCollection, type LibrarySourceId } from './types';

/** What a saved source id may point to; ids pointing elsewhere are stale. */
export interface KnownSources {
	collections: readonly LibraryCollection[];
	indexedFolders: readonly string[];
}

/** Built-in id, or an id of the `collection:` / `pack:` form (existence not checked). */
export function hasSourceShape(value: string): value is LibrarySourceId {
	return (
		isLibraryBuiltInSourceId(value) ||
		collectionIdOfSource(value as LibrarySourceId) !== null ||
		packFolderOfSource(value as LibrarySourceId) !== null
	);
}

export function isKnownSource(source: LibrarySourceId, known: KnownSources): boolean {
	if (isLibraryBuiltInSourceId(source)) return true;
	const collectionId = collectionIdOfSource(source);
	if (collectionId) return known.collections.some((collection) => collection.id === collectionId);
	const folder = packFolderOfSource(source);
	return folder !== null && known.indexedFolders.includes(folder);
}

export function withoutSource<T extends Partial<Record<string, LibrarySourceId>>>(
	remembered: T,
	source: LibrarySourceId
): T {
	return Object.fromEntries(
		Object.entries(remembered).filter(([, value]) => value !== source)
	) as T;
}
