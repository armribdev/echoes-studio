import type { LibraryAsset } from './types';

/** Case-insensitive name substring match (Phase 1 search stub). */
export function filterAssetsByQuery(assets: LibraryAsset[], query: string): LibraryAsset[] {
	const q = query.trim().toLowerCase();
	if (!q) return assets;
	return assets.filter((a) => a.name.toLowerCase().includes(q));
}
