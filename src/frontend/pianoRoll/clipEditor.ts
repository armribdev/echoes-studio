import { get } from 'svelte/store';
import type { Track, MidiClip } from '../../types/global';
import type { SelectionState } from '../../stores/selection';
import { pianoRollStore } from '../../stores/pianoRoll';
import { bottomPanel } from '../../stores/bottomPanel';
import { keyboardFocus } from '../../stores/keyboardFocus';
import { tempo } from '../../stores/tempo';
import { computePreviewPanelHeight } from './previewLayout';

const TRACK_COLORS = [
	'#4d96ff', '#ff6b6b', '#6bcb77', '#ffd93d',
	'#c77dff', '#ff9a3c', '#00c9a7', '#ff6db6',
];

export interface ResolvedMidiClip {
	track: Track;
	trackIndex: number;
	clip: MidiClip;
}

export function trackColor(trackIndex: number): string {
	return TRACK_COLORS[trackIndex % TRACK_COLORS.length];
}

function clipDurationBeats(clip: MidiClip): number {
	const bpm = get(tempo).bpm;
	return ((clip.end - clip.start) / 60) * bpm;
}

function persistEditLayout() {
	const wasEdit = get(keyboardFocus) === 'piano-roll';
	if (!wasEdit) return;
	bottomPanel.saveEditHeight(get(bottomPanel).height);
	pianoRollStore.saveEditZoom(get(pianoRollStore).zoom);
}

export function resolveActiveMidiClip(
	tracks: Track[],
	state: SelectionState,
): ResolvedMidiClip | null {
	const { primaryId, ids } = state;
	if (ids.size === 0) return null;

	const tryResolve = (clipId: string): ResolvedMidiClip | null => {
		for (let trackIndex = 0; trackIndex < tracks.length; trackIndex++) {
			const track = tracks[trackIndex];
			const clip = track.midi_clips?.find((c) => c.id === clipId);
			if (clip) return { track, trackIndex, clip };
		}
		return null;
	};

	if (primaryId && ids.has(primaryId)) {
		const resolved = tryResolve(primaryId);
		if (resolved) return resolved;
	}

	for (const clipId of ids) {
		const resolved = tryResolve(clipId);
		if (resolved) return resolved;
	}

	return null;
}

export function showMidiClipPreview(
	track: Track,
	clip: MidiClip,
	color: string,
) {
	const pr = get(pianoRollStore);
	const bp = get(bottomPanel);
	const alreadyPreviewing =
		bp.open &&
		pr.open &&
		pr.trackId === track.id &&
		pr.clipId === clip.id &&
		get(keyboardFocus) !== 'piano-roll';
	if (alreadyPreviewing) return;

	persistEditLayout();
	keyboardFocus.setTimeline();
	const duration = clipDurationBeats(clip);
	const clipNotes = clip.notes ?? [];
	pianoRollStore.clearSelection();
	pianoRollStore.loadClip(
		track.id,
		clip.id,
		track.name,
		color,
		clipNotes,
		duration,
	);
	bottomPanel.openTab('pianoroll');
	bottomPanel.setPreviewAutoHeight(computePreviewPanelHeight(clipNotes));
}

/** Ouvre l'aperçu docké si nécessaire (clic clip, panneau fermé après Échap, etc.). */
export function ensureClipPreview(track: Track, clip: MidiClip, color: string) {
	if (get(keyboardFocus) === 'piano-roll') {
		focusMidiClipForEdit(track, clip, color);
		return;
	}
	showMidiClipPreview(track, clip, color);
}

/** Passe du panneau docké (aperçu) au mode édition pour le clip déjà chargé. */
export function enterEditFromPreview() {
	const pr = get(pianoRollStore);
	if (!pr.open || get(keyboardFocus) === 'piano-roll') return;
	keyboardFocus.setPianoRoll();
	bottomPanel.restoreEditHeight();
	pianoRollStore.restoreEditZoom();
}

export function focusMidiClipForEdit(
	track: Track,
	clip: MidiClip,
	color: string,
) {
	pianoRollStore.loadClip(
		track.id,
		clip.id,
		track.name,
		color,
		clip.notes ?? [],
		clipDurationBeats(clip),
	);
	bottomPanel.openTab('pianoroll');
	keyboardFocus.setPianoRoll();
	bottomPanel.restoreEditHeight();
	pianoRollStore.restoreEditZoom();
}

/** Leave edit mode while keeping the docked preview open. */
export function exitEditToPreview() {
	persistEditLayout();
	keyboardFocus.setTimeline();
	pianoRollStore.clearSelection();
	const pr = get(pianoRollStore);
	bottomPanel.setPreviewAutoHeight(computePreviewPanelHeight(pr.notes));
}

/**
 * Échap en cascade : désélection → aperçu → fermeture.
 * Un seul point d'entrée pour éviter les doubles handlers window.
 */
export function handleEditorEscapeKey(): boolean {
	const bp = get(bottomPanel);
	if (!bp.open) return false;

	const pr = get(pianoRollStore);
	if (!pr.open) {
		closeEditor();
		return true;
	}

	if (get(keyboardFocus) === 'piano-roll') {
		if (pr.selectedNoteIds.size > 0) {
			pianoRollStore.clearSelection();
		} else {
			exitEditToPreview();
		}
		return true;
	}

	closeEditor();
	return true;
}

export function closeEditor() {
	bottomPanel.close();
	pianoRollStore.closePianoRoll();
	keyboardFocus.setTimeline();
}

/** Ferme le piano roll si la sélection ne contient plus de clip MIDI actif. */
export function syncPianoRollToSelection(tracks: Track[], state: SelectionState) {
	if (get(keyboardFocus) === 'piano-roll') return;
	if (resolveActiveMidiClip(tracks, state)) return;
	if (get(bottomPanel).open) closeEditor();
	else if (get(pianoRollStore).open) pianoRollStore.closePianoRoll();
}

/**
 * Tab : monte d'un cran dans la pile éditeur (miroir d'Escape).
 *   A (panel fermé) + clip MIDI sélectionné → C (édition directe)
 *   B (preview, focus arrangement)          → C (édition)
 *   C (édition)                             → B (preview, focus arrangement)
 *   A + rien à éditer                       → no-op
 */
export function cycleEditorFocus(tracks: Track[], state: SelectionState) {
	const bp = get(bottomPanel);

	if (!bp.open) {
		const resolved = resolveActiveMidiClip(tracks, state);
		if (!resolved) return;
		focusMidiClipForEdit(resolved.track, resolved.clip, trackColor(resolved.trackIndex));
		return;
	}

	if (get(keyboardFocus) === 'piano-roll') {
		exitEditToPreview();
		return;
	}

	// Panel ouvert, focus = timeline → entrer en édition.
	if (get(pianoRollStore).open) {
		enterEditFromPreview();
		return;
	}

	// Preview ouvert sans clip chargé (rare : panel ouvert via UI sans sélection).
	const resolved = resolveActiveMidiClip(tracks, state);
	if (resolved) focusMidiClipForEdit(resolved.track, resolved.clip, trackColor(resolved.trackIndex));
}
