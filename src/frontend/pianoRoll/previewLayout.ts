import type { MidiNote } from '../../types/global';
import { PREVIEW_PANEL_MIN, PREVIEW_PANEL_RESIZE_MAX } from '../../stores/bottomPanel';
import { NOTE_H } from './coords';
import { RULER_HEIGHT } from '../timeRuler/constants';

/** Barre chrome unique du piano roll (sans entête panneau séparé). */
export const PIANO_ROLL_TOOLBAR_H = 30;

/** Chrome hors grille : toolbar + règle. */
export const PREVIEW_CHROME_H = PIANO_ROLL_TOOLBAR_H + RULER_HEIGHT;

export function getPreviewPitchRange(noteList: MidiNote[]): { min: number; max: number } {
	if (noteList.length === 0) return { min: 57, max: 72 };
	const pitches = noteList.map((n) => n.pitch);
	return {
		min: Math.max(0, Math.min(...pitches) - 3),
		max: Math.min(127, Math.max(...pitches) + 3)
	};
}

/** Hauteur idéale du panneau pour afficher toutes les hauteurs au zoom vertical 1 (NOTE_H). */
export function computeIdealPreviewPanelHeight(noteList: MidiNote[]): number {
	const { min, max } = getPreviewPitchRange(noteList);
	const pitchCount = Math.max(1, max - min + 1);
	return PREVIEW_CHROME_H + pitchCount * NOTE_H;
}

/** Plafond de redimensionnement manuel (zoom vertical 1 si possible, sinon plafond global). */
export function computePreviewResizeMax(noteList: MidiNote[]): number {
	const idealPanelH = computeIdealPreviewPanelHeight(noteList);
	return Math.max(PREVIEW_PANEL_MIN, Math.min(PREVIEW_PANEL_RESIZE_MAX, idealPanelH));
}

/** Hauteur du panneau inférieur pour l'aperçu (ouverture auto / fit initial). */
export function computePreviewPanelHeight(noteList: MidiNote[]): number {
	return computePreviewResizeMax(noteList);
}

/**
 * Hauteur de ligne pour la hauteur panneau donnée.
 * Zoom vertical 1 (= NOTE_H) quand l'espace le permet ; sinon réduction pour tout afficher.
 */
export function computePreviewRowH(noteList: MidiNote[], panelHeight: number): number {
	const { min, max } = getPreviewPitchRange(noteList);
	const pitchCount = Math.max(1, max - min + 1);
	const gridH = Math.max(40, panelHeight - PREVIEW_CHROME_H);
	const natural = gridH / pitchCount;
	if (natural >= NOTE_H) return NOTE_H;
	return Math.max(4, natural);
}

export function previewPitchCount(noteList: MidiNote[]): number {
	const { min, max } = getPreviewPitchRange(noteList);
	return Math.max(1, max - min + 1);
}
