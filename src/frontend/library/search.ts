import type { LibraryAsset, LibraryKindFilter } from './types';

/** Case-insensitive substring match on the name or on one of the tags. */
export function filterAssetsByQuery(assets: LibraryAsset[], query: string): LibraryAsset[] {
	const q = query.trim().toLowerCase();
	if (!q) return assets;
	return assets.filter(
		(a) => a.name.toLowerCase().includes(q) || (a.tags?.some((tag) => tag.includes(q)) ?? false)
	);
}

export function matchesKindFilter(asset: LibraryAsset, kind: LibraryKindFilter): boolean {
	return kind === 'audio'
		? asset.kind === 'one_shot' || asset.kind === 'loop'
		: asset.kind === kind;
}

export function filterAssetsByKind(
	assets: LibraryAsset[],
	kind: LibraryKindFilter | null
): LibraryAsset[] {
	return kind ? assets.filter((asset) => matchesKindFilter(asset, kind)) : assets;
}
