/** Library asset kinds (ADR-023 / library-browser-ux.md). */
export type LibraryAssetKind = 'one_shot' | 'loop' | 'midi_pattern' | 'chord_progression';

/** Organisational + content sources (Phase 1 audio + Phase 3 musical). */
export type LibraryBuiltInSourceId =
	| 'recents'
	| 'favorites'
	| 'audio'
	| 'chords'
	| 'midi'
	| 'drums';

/** A saved search shown as a source: `collection:<id>`. */
export type LibraryCollectionSourceId = `collection:${string}`;

/** An indexed folder browsed on its own: `pack:<folder path>`. */
export type LibraryPackSourceId = `pack:${string}`;

export type LibrarySourceId =
	| LibraryBuiltInSourceId
	| LibraryCollectionSourceId
	| LibraryPackSourceId;

/** How results are laid out: one row per sound, or a grid of tiles. */
export type LibraryResultsView = 'list' | 'grid';

/** Coarse asset type filter: loops and one-shots are both « audio ». */
export type LibraryKindFilter = 'audio' | 'midi_pattern' | 'chord_progression';

export interface LibraryCollection {
	id: string;
	name: string;
	query: string;
	kind: LibraryKindFilter | null;
}

/** Open contexts for the insert pop-up (library-browser-ux.md §5). */
export type LibraryOpenContext =
	| 'track_empty'
	| 'track_audio_lane'
	| 'track_midi_lane'
	| 'playhead_insert';

export const LIBRARY_OPEN_CONTEXTS: LibraryOpenContext[] = [
	'track_empty',
	'track_audio_lane',
	'track_midi_lane',
	'playhead_insert'
];

/** Source the user chose to open each context on (opt-in; absent = factory default). */
export type LibraryContextSources = Partial<Record<LibraryOpenContext, LibrarySourceId>>;

export function isLibraryResultsView(value: unknown): value is LibraryResultsView {
	return value === 'list' || value === 'grid';
}

export function isLibraryOpenContext(value: string): value is LibraryOpenContext {
	return (LIBRARY_OPEN_CONTEXTS as readonly string[]).includes(value);
}

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

export const LIBRARY_SOURCE_ORDER: LibraryBuiltInSourceId[] = [
	'recents',
	'favorites',
	'audio',
	'chords',
	'midi',
	'drums'
];

export const LIBRARY_SOURCE_LABELS: Record<LibraryBuiltInSourceId, string> = {
	recents: 'Récents',
	favorites: 'Favoris',
	audio: 'Audio',
	chords: 'Accords',
	midi: 'MIDI',
	drums: 'Drums'
};

export const CONTENT_SOURCES: LibraryBuiltInSourceId[] = ['audio', 'chords', 'midi', 'drums'];

export const LIBRARY_KIND_FILTERS: LibraryKindFilter[] = [
	'audio',
	'midi_pattern',
	'chord_progression'
];

export function isLibraryBuiltInSourceId(value: string): value is LibraryBuiltInSourceId {
	return (LIBRARY_SOURCE_ORDER as readonly string[]).includes(value);
}

export function isLibraryKindFilter(value: unknown): value is LibraryKindFilter {
	return (LIBRARY_KIND_FILTERS as unknown[]).includes(value);
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
		origin: 'file'
	};
}

export function assetFromScanEntry(entry: LibraryScanEntry): LibraryAsset {
	return {
		id: assetIdFromPath(entry.path),
		kind: 'one_shot',
		path: entry.path,
		name: entry.name,
		extension: entry.extension.toLowerCase(),
		origin: 'file'
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
