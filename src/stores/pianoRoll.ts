import { writable, derived, get } from 'svelte/store';
import type { MidiNote } from '../types/midi';

// ── Types ─────────────────────────────────────────────────────────────────────

export type PianoRollMode = 'draw' | 'select' | 'erase';

export interface PianoRollState {
	open: boolean;
	trackId: string | null;
	trackName: string;
	trackColor: string;
	/** Notes par piste : trackId → MidiNote[] */
	notesByTrack: Record<string, MidiNote[]>;
	selectedNoteIds: Set<string>;
	mode: PianoRollMode;
	quantize: number; // en beats, ex. 0.25 = 1/16
	zoom: number;     // px par beat
	/** Historique undo (snapshots des notes de la piste courante) */
	past: MidiNote[][];
	future: MidiNote[][];
}

const INITIAL: PianoRollState = {
	open: false,
	trackId: null,
	trackName: '',
	trackColor: '#4d96ff',
	notesByTrack: {},
	selectedNoteIds: new Set(),
	mode: 'draw',
	quantize: 0.25, // 1/16
	zoom: 80,
	past: [],
	future: [],
};

// ── Store ─────────────────────────────────────────────────────────────────────

function createPianoRollStore() {
	const { subscribe, set, update } = writable<PianoRollState>(INITIAL);

	// ── Helpers ───────────────────────────────────────────────────────────────

	function snapshot(state: PianoRollState): MidiNote[] {
		if (!state.trackId) return [];
		return [...(state.notesByTrack[state.trackId] ?? [])];
	}

	function pushHistory(state: PianoRollState): PianoRollState {
		return {
			...state,
			past: [...state.past.slice(-49), snapshot(state)],
			future: [],
		};
	}

	function currentNotes(state: PianoRollState): MidiNote[] {
		if (!state.trackId) return [];
		return state.notesByTrack[state.trackId] ?? [];
	}

	function setNotes(state: PianoRollState, notes: MidiNote[]): PianoRollState {
		if (!state.trackId) return state;
		return {
			...state,
			notesByTrack: { ...state.notesByTrack, [state.trackId]: notes },
		};
	}

	// ── Public API ────────────────────────────────────────────────────────────

	return {
		subscribe,

		openPianoRoll(trackId: string, trackName: string, trackColor: string) {
			update((s) => ({
				...s,
				open: true,
				trackId,
				trackName,
				trackColor,
				selectedNoteIds: new Set(),
				past: [],
				future: [],
			}));
		},

		closePianoRoll() {
			update((s) => ({
				...s,
				open: false,
				selectedNoteIds: new Set(),
				past: [],
				future: [],
			}));
		},

		setMode(mode: PianoRollMode) {
			update((s) => ({ ...s, mode, selectedNoteIds: new Set() }));
		},

		setQuantize(beats: number) {
			update((s) => ({ ...s, quantize: beats }));
		},

		setZoom(zoom: number) {
			update((s) => ({ ...s, zoom: Math.max(20, Math.min(400, zoom)) }));
		},

		// ── Note CRUD ─────────────────────────────────────────────────────────

		addNote(pitch: number, start: number, duration: number, velocity = 100): string {
			const id = crypto.randomUUID().slice(0, 8);
			update((s) => {
				const saved = pushHistory(s);
				const notes = [...currentNotes(saved), { id, pitch, start, duration, velocity }];
				return setNotes(saved, notes);
			});
			return id;
		},

		moveNote(id: string, pitch: number, start: number) {
			update((s) => {
				const saved = pushHistory(s);
				const notes = currentNotes(saved).map((n) =>
					n.id === id ? { ...n, pitch: clamp(pitch, 0, 127), start: Math.max(0, start) } : n
				);
				return setNotes(saved, notes);
			});
		},

		moveSelectedNotes(deltaPitch: number, deltaStart: number) {
			update((s) => {
				if (s.selectedNoteIds.size === 0) return s;
				const saved = pushHistory(s);
				const notes = currentNotes(saved).map((n) =>
					s.selectedNoteIds.has(n.id)
						? {
								...n,
								pitch: clamp(n.pitch + deltaPitch, 0, 127),
								start: Math.max(0, n.start + deltaStart),
							}
						: n
				);
				return setNotes(saved, notes);
			});
		},

		resizeNote(id: string, duration: number) {
			update((s) => {
				const saved = pushHistory(s);
				const notes = currentNotes(saved).map((n) =>
					n.id === id ? { ...n, duration: Math.max(0.0156, duration) } : n
				);
				return setNotes(saved, notes);
			});
		},

		deleteNote(id: string) {
			update((s) => {
				const saved = pushHistory(s);
				const notes = currentNotes(saved).filter((n) => n.id !== id);
				const sel = new Set(saved.selectedNoteIds);
				sel.delete(id);
				return { ...setNotes(saved, notes), selectedNoteIds: sel };
			});
		},

		deleteSelectedNotes() {
			update((s) => {
				if (s.selectedNoteIds.size === 0) return s;
				const saved = pushHistory(s);
				const notes = currentNotes(saved).filter((n) => !s.selectedNoteIds.has(n.id));
				return { ...setNotes(saved, notes), selectedNoteIds: new Set() };
			});
		},

		setVelocity(id: string, velocity: number) {
			update((s) => {
				const saved = pushHistory(s);
				const notes = currentNotes(saved).map((n) =>
					n.id === id ? { ...n, velocity: clamp(velocity, 1, 127) } : n
				);
				return setNotes(saved, notes);
			});
		},

		setVelocityMultiple(ids: Set<string>, velocity: number) {
			update((s) => {
				const saved = pushHistory(s);
				const notes = currentNotes(saved).map((n) =>
					ids.has(n.id) ? { ...n, velocity: clamp(velocity, 1, 127) } : n
				);
				return setNotes(saved, notes);
			});
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
			update((s) => {
				const ids = currentNotes(s).map((n) => n.id);
				return { ...s, selectedNoteIds: new Set(ids) };
			});
		},

		clearSelection() {
			update((s) => ({ ...s, selectedNoteIds: new Set() }));
		},

		// ── Undo / Redo ───────────────────────────────────────────────────────

		undo() {
			update((s) => {
				if (s.past.length === 0) return s;
				const past = [...s.past];
				const prev = past.pop()!;
				const future = [snapshot(s), ...s.future];
				const next = { ...s, past, future, selectedNoteIds: new Set<string>() };
				return setNotes(next, prev);
			});
		},

		redo() {
			update((s) => {
				if (s.future.length === 0) return s;
				const future = [...s.future];
				const next = future.shift()!;
				const past = [...s.past, snapshot(s)];
				const st = { ...s, past, future, selectedNoteIds: new Set<string>() };
				return setNotes(st, next);
			});
		},

		// ── Accessors ─────────────────────────────────────────────────────────

		getNotes(): MidiNote[] {
			const s = get({ subscribe });
			return currentNotes(s);
		},
	};
}

function clamp(v: number, min: number, max: number) {
	return Math.max(min, Math.min(max, v));
}

export const pianoRollStore = createPianoRollStore();

export const pianoRollNotes = derived(pianoRollStore, ($s) => {
	if (!$s.trackId) return [] as MidiNote[];
	return $s.notesByTrack[$s.trackId] ?? ([] as MidiNote[]);
});
