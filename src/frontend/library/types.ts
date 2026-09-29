/** Library asset kinds (ADR-023 / library-browser-ux.md). */
export type LibraryAssetKind = 'one_shot' | 'loop' | 'midi_pattern' | 'chord_progression';

/** Organisational + content sources (Phase 1 audio + Phase 3 musical). */
export type LibrarySourceId = 'recents' | 'favorites' | 'audio' | 'chords' | 'midi' | 'drums';

/** Open contexts for the insert pop-up (library-browser-ux.md §5). */
export type LibraryOpenContext =
	| 'track_empty'
	| 'track_audio_lane'
	| 'track_midi_lane'
	| 'playhead_insert';

/** Drop-target lane focus — not a stored track kind (UX §4). */
export type LaneFocus = 'audio' | 'midi' | 'neutral';

export type LibraryIndexerState = 'idle' | 'scanning' | 'ready' | 'error';

export interface LibraryMidiNote {
	pitch: number;
	start: number;
	duration: number;
	velocity: number;
}

export interface LibraryChordStep {
	symbol: string;
	/** MIDI pitch of the chord root (e.g. 60 = C4). */
	rootPitch: number;
	quality: LibraryChordQuality;
	startBeat: number;
	durationBeats: number;
}

export type LibraryChordQuality =
	| 'maj'
	| 'min'
	| 'dom7'
	| 'min7'
	| 'maj7'
	| 'sus2'
	| 'sus4'
	| 'dim'
	| 'aug';

export interface LibraryAsset {
	id: string;
	kind: LibraryAssetKind;
	path: string;
	name: string;
	extension: string;
	/** Seconds, when known (scan or insert). */
	durationSec?: number;
	folderPath?: string;
	/** Pattern length in beats (MIDI / chords). */
	lengthBeats?: number;
	/** Declared key root for preview transpose (e.g. "C"). */
	keyRoot?: string;
	/** Pre-expanded notes for `midi_pattern`; chords expand at insert/preview. */
	notes?: LibraryMidiNote[];
	/** Chord steps for `chord_progression`. */
	chords?: LibraryChordStep[];
	origin?: 'factory' | 'file';
	tags?: string[];
}

export interface LibraryScanEntry {
	path: string;
	name: string;
	extension: string;
}

export const AUDIO_EXTENSIONS = ['wav', 'mp3', 'flac', 'ogg', 'oga'] as const;

export const LIBRARY_DRAG_MIME = 'application/x-echoes-library-asset';

export const LIBRARY_SOURCE_ORDER: LibrarySourceId[] = [
	'recents',
	'favorites',
	'audio',
	'chords',
	'midi',
	'drums',
];

export const LIBRARY_SOURCE_LABELS: Record<LibrarySourceId, string> = {
	recents: 'Récents',
	favorites: 'Favoris',
	audio: 'Audio',
	chords: 'Accords',
	midi: 'MIDI',
	drums: 'Drums',
};

export const CONTENT_SOURCES: LibrarySourceId[] = ['audio', 'chords', 'midi', 'drums'];

export function isLibrarySourceId(value: string): value is LibrarySourceId {
	return (LIBRARY_SOURCE_ORDER as readonly string[]).includes(value);
}

export function isMidiLibraryKind(kind: LibraryAssetKind): boolean {
	return kind === 'midi_pattern' || kind === 'chord_progression';
}

export function isAudioLibraryKind(kind: LibraryAssetKind): boolean {
	return kind === 'one_shot' || kind === 'loop';
}

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
		origin: 'file',
	};
}

export function assetFromScanEntry(entry: LibraryScanEntry): LibraryAsset {
	return {
		id: assetIdFromPath(entry.path),
		kind: 'one_shot',
		path: entry.path,
		name: entry.name,
		extension: entry.extension.toLowerCase(),
		origin: 'file',
	};
}

export function formatDuration(sec: number | undefined): string {
	if (sec == null || !Number.isFinite(sec) || sec < 0) return '—';
	if (sec < 60) return `${sec.toFixed(1)}s`;
	const m = Math.floor(sec / 60);
	const s = Math.floor(sec % 60);
	return `${m}:${s.toString().padStart(2, '0')}`;
}

export function formatBeats(beats: number | undefined): string {
	if (beats == null || !Number.isFinite(beats) || beats <= 0) return '—';
	if (Number.isInteger(beats)) return `${beats}b`;
	return `${beats.toFixed(1)}b`;
}
