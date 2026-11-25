import type { LibraryAsset } from './types';

/** Case-insensitive substring match on the name or on one of the tags. */
export function filterAssetsByQuery(assets: LibraryAsset[], query: string): LibraryAsset[] {
	const q = query.trim().toLowerCase();
	if (!q) return assets;
	return assets.filter(
		(a) => a.name.toLowerCase().includes(q) || (a.tags?.some((tag) => tag.includes(q)) ?? false)
	);
}
