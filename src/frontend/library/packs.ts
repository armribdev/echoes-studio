import { fileNameFromPath, type LibraryPackSourceId, type LibrarySourceId } from './types';

const SOURCE_PREFIX = 'pack:';

/** A pack is an indexed folder: its source id carries the folder path. */
export function packSourceId(folderPath: string): LibraryPackSourceId {
	return `${SOURCE_PREFIX}${folderPath}`;
}

export function packFolderOfSource(source: LibrarySourceId): string | null {
	return source.startsWith(SOURCE_PREFIX) ? source.slice(SOURCE_PREFIX.length) : null;
}

export function packLabel(folderPath: string): string {
	const trimmed = folderPath.replace(/[\\/]+$/, '');
	return fileNameFromPath(trimmed) || folderPath;
}

export function isInsideFolder(path: string, folderPath: string): boolean {
	const normalize = (p: string) => p.replace(/\\/g, '/').toLowerCase();
	const base = normalize(folderPath).replace(/\/+$/, '');
	return normalize(path).startsWith(`${base}/`);
}

export function packAssetIds(audioIds: readonly string[], folderPath: string): string[] {
	return audioIds.filter((id) => isInsideFolder(id, folderPath));
}

/** Indexed sounds that only this folder brought in (another folder may contain them too). */
export function idsOnlyInFolder(
	audioIds: readonly string[],
	folderPath: string,
	otherFolders: readonly string[]
): string[] {
	return packAssetIds(audioIds, folderPath).filter(
		(id) => !otherFolders.some((other) => isInsideFolder(id, other))
	);
}
