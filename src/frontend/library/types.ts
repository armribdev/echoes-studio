/** Phase 1 library asset kinds (ADR-023 / library-browser-ux.md). */
export type LibraryAssetKind = 'one_shot' | 'loop';

/** Organisational + content sources shipped in Phase 1. */
export type LibrarySourceId = 'recents' | 'favorites' | 'audio';

export type LibraryIndexerState = 'idle' | 'scanning' | 'ready' | 'error';

export interface LibraryAsset {
	id: string;
	kind: LibraryAssetKind;
	path: string;
	name: string;
	extension: string;
	/** Seconds, when known (scan or insert). */
	durationSec?: number;
	folderPath?: string;
}

export interface LibraryScanEntry {
	path: string;
	name: string;
	extension: string;
}

export const AUDIO_EXTENSIONS = ['wav', 'mp3', 'flac', 'ogg', 'oga'] as const;

export const LIBRARY_DRAG_MIME = 'application/x-echoes-library-asset';

export const LIBRARY_SOURCE_ORDER: LibrarySourceId[] = ['recents', 'favorites', 'audio'];

export const LIBRARY_SOURCE_LABELS: Record<LibrarySourceId, string> = {
	recents: 'Récents',
	favorites: 'Favoris',
	audio: 'Audio',
};

export function assetIdFromPath(path: string): string {
	return path;
}

export function fileNameFromPath(path: string): string {
	return path.split(/[\\/]/).pop() ?? path;
}

export function fileExtension(path: string): string {
	const name = fileNameFromPath(path);
	const dot = name.lastIndexOf('.');
	return dot >= 0 ? name.slice(dot + 1).toLowerCase() : '';
}

export function isAudioPath(path: string): boolean {
	return (AUDIO_EXTENSIONS as readonly string[]).includes(fileExtension(path));
}

export function assetFromPath(path: string, durationSec?: number): LibraryAsset {
	const extension = fileExtension(path);
	return {
		id: assetIdFromPath(path),
		kind: 'one_shot',
		path,
		name: fileNameFromPath(path),
		extension,
		durationSec,
	};
}

export function assetFromScanEntry(entry: LibraryScanEntry): LibraryAsset {
	return {
		id: assetIdFromPath(entry.path),
		kind: 'one_shot',
		path: entry.path,
		name: entry.name,
		extension: entry.extension.toLowerCase(),
	};
}

export function formatDuration(sec: number | undefined): string {
	if (sec == null || !Number.isFinite(sec) || sec < 0) return '—';
	if (sec < 60) return `${sec.toFixed(1)}s`;
	const m = Math.floor(sec / 60);
	const s = Math.floor(sec % 60);
	return `${m}:${s.toString().padStart(2, '0')}`;
}
