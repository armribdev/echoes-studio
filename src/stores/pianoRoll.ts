import { writable, derived, get } from 'svelte/store';
import type { MidiNote } from '../types/global';

// ── Types ─────────────────────────────────────────────────────────────────────

export type PianoRollMode = 'draw' | 'select' | 'erase';

export interface EditScrollPosition {
	scrollLeft: number;
	scrollTop: number;
}

export interface PianoRollState {
	open: boolean;
	trackId: string | null;
	clipId: string | null;
	trackName: string;
	trackColor: string;
	notes: MidiNote[];
	selectedNoteIds: Set<string>;
	mode: PianoRollMode;
	quantize: number; // in beats, e.g. 0.25 = 1/16
	zoom: number;     // px per beat
	clipDurationBeats: number;
	/** Zoom preserved when leaving edit mode. */
	savedEditZoom: number;
	/** Scroll édition mémorisé par clip (trackId:clipId). */
	editScrollByClipKey: Record<string, EditScrollPosition>;
}

const INITIAL: PianoRollState = {
	open: false,
	trackId: null,
	clipId: null,
	trackName: '',
	trackColor: '#4d96ff',
	notes: [],
	selectedNoteIds: new Set(),
	mode: 'draw',
	quantize: 0.25,
	zoom: 80,
	clipDurationBeats: 16,
	savedEditZoom: 80,
	editScrollByClipKey: {},
};

// ── Store ─────────────────────────────────────────────────────────────────────

function createPianoRollStore() {
	const { subscribe, set, update } = writable<PianoRollState>(INITIAL);

	// ── Public API ────────────────────────────────────────────────────────────

	return {
		subscribe,

		openPianoRoll(trackId: string, clipId: string, trackName: string, trackColor: string, initialNotes: MidiNote[] = []) {
			this.loadClip(trackId, clipId, trackName, trackColor, initialNotes, 16);
		},

		loadClip(
			trackId: string,
			clipId: string,
			trackName: string,
			trackColor: string,
			initialNotes: MidiNote[] = [],
			clipDurationBeats = 16,
		) {
			update((s) => {
				const sameClip =
					s.open &&
					s.trackId === trackId &&
					s.clipId === clipId &&
					s.clipDurationBeats === clipDurationBeats &&
					s.trackName === trackName &&
					s.trackColor === trackColor &&
					s.notes === initialNotes;
				if (sameClip) return s;
				const keepSelection = s.open && s.trackId === trackId && s.clipId === clipId;
				return {
					...s,
					open: true,
					trackId,
					clipId,
					trackName,
					trackColor,
					notes: initialNotes,
					clipDurationBeats,
					selectedNoteIds: keepSelection ? s.selectedNoteIds : new Set(),
				};
			});
		},

		closePianoRoll() {
			update((s) => ({
				...s,
				open: false,
				clipId: null,
				trackId: null,
				notes: [],
				selectedNoteIds: new Set(),
			}));
		},

		/** Sync notes from a Rust event (undo/redo/PROJECT_RESET). */
		syncNotes(notes: MidiNote[]) {
			update((s) => {
				if (s.notes.length === notes.length && s.notes.every((n, i) => n.id === notes[i]?.id)) {
					return s;
				}
				return { ...s, notes: [...notes] };
			});
		},

		setMode(mode: PianoRollMode) {
			update((s) => ({ ...s, mode, selectedNoteIds: new Set() }));
		},

		setQuantize(beats: number) {
			update((s) => ({ ...s, quantize: beats }));
		},

		setZoom(zoom: number) {
			update((s) => {
				const z = Math.max(20, Math.min(400, zoom));
				if (s.zoom === z) return s;
				return { ...s, zoom: z };
			});
		},

		saveEditZoom(zoom: number) {
			update((s) => ({ ...s, savedEditZoom: Math.max(20, Math.min(400, zoom)) }));
		},

		restoreEditZoom() {
			update((s) => (s.zoom === s.savedEditZoom ? s : { ...s, zoom: s.savedEditZoom }));
		},

		saveEditScroll(trackId: string, clipId: string, scrollLeft: number, scrollTop: number) {
			const key = `${trackId}:${clipId}`;
			const pos = {
				scrollLeft: Math.max(0, scrollLeft),
				scrollTop: Math.max(0, scrollTop),
			};
			update((s) => {
				const prev = s.editScrollByClipKey[key];
				if (prev?.scrollLeft === pos.scrollLeft && prev?.scrollTop === pos.scrollTop) return s;
				return {
					...s,
					editScrollByClipKey: { ...s.editScrollByClipKey, [key]: pos },
				};
			});
		},

		getEditScroll(trackId: string | null, clipId: string | null): EditScrollPosition | null {
			if (!trackId || !clipId) return null;
			return get({ subscribe }).editScrollByClipKey[`${trackId}:${clipId}`] ?? null;
		},

		// ── Note CRUD (local-only, commit via setMidiNotes on mouseup) ─────────

		addNote(pitch: number, start: number, duration: number, velocity = 100): string {
			const id = crypto.randomUUID().slice(0, 8);
			update((s) => ({
				...s,
				notes: [...s.notes, { id, pitch, start, duration, velocity }],
			}));
			return id;
		},

		moveNote(id: string, pitch: number, start: number) {
			update((s) => ({
				...s,
				notes: s.notes.map((n) =>
					n.id === id ? { ...n, pitch: clamp(pitch, 0, 127), start: Math.max(0, start) } : n,
				),
			}));
		},

		moveSelectedNotes(deltaPitch: number, deltaStart: number) {
			update((s) => {
				if (s.selectedNoteIds.size === 0) return s;
				return {
					...s,
					notes: s.notes.map((n) =>
						s.selectedNoteIds.has(n.id)
							? {
									...n,
									pitch: clamp(n.pitch + deltaPitch, 0, 127),
									start: Math.max(0, n.start + deltaStart),
								}
							: n,
					),
				};
			});
		},

		resizeNote(id: string, duration: number) {
			update((s) => ({
				...s,
				notes: s.notes.map((n) =>
					n.id === id ? { ...n, duration: Math.max(0.0156, duration) } : n,
				),
			}));
		},

		/** Remplace toutes les notes (commit drag local, sync batch). */
		setNotes(notes: MidiNote[]) {
			update((s) => ({ ...s, notes: [...notes] }));
		},

		deleteNote(id: string) {
			update((s) => {
				const sel = new Set(s.selectedNoteIds);
				sel.delete(id);
				return { ...s, notes: s.notes.filter((n) => n.id !== id), selectedNoteIds: sel };
			});
		},

		deleteSelectedNotes() {
			update((s) => {
				if (s.selectedNoteIds.size === 0) return s;
				return {
					...s,
					notes: s.notes.filter((n) => !s.selectedNoteIds.has(n.id)),
					selectedNoteIds: new Set(),
				};
			});
		},

		setVelocity(id: string, velocity: number) {
			update((s) => ({
				...s,
				notes: s.notes.map((n) =>
					n.id === id ? { ...n, velocity: clamp(velocity, 1, 127) } : n,
				),
			}));
		},

		setVelocityMultiple(ids: Set<string>, velocity: number) {
			update((s) => ({
				...s,
				notes: s.notes.map((n) =>
					ids.has(n.id) ? { ...n, velocity: clamp(velocity, 1, 127) } : n,
				),
			}));
		},

		// ── Selection ─────────────────────────────────────────────────────────

		selectNote(id: string, additive = false) {
			update((s) => {
				const sel = additive ? new Set(s.selectedNoteIds) : new Set<string>();
				sel.has(id) && additive ? sel.delete(id) : sel.add(id);
				return { ...s, selectedNoteIds: sel };
			});
		},

		selectNotes(ids: string[]) {
			update((s) => ({ ...s, selectedNoteIds: new Set(ids) }));
		},

		selectAll() {
			update((s) => ({ ...s, selectedNoteIds: new Set(s.notes.map((n) => n.id)) }));
		},

		clearSelection() {
			update((s) => (s.selectedNoteIds.size === 0 ? s : { ...s, selectedNoteIds: new Set() }));
		},

		// ── Accessors ─────────────────────────────────────────────────────────

		getNotes(): MidiNote[] {
			return get({ subscribe }).notes;
		},
	};
}

function clamp(v: number, min: number, max: number) {
	return Math.max(min, Math.min(max, v));
}

export const pianoRollStore = createPianoRollStore();

export const pianoRollNotes = derived(pianoRollStore, ($s) => $s.notes);

export const pianoRollSelection = derived(pianoRollStore, ($s) => $s.selectedNoteIds);
