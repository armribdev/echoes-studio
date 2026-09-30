import type { MidiNote } from '../../types/global';
import { NOTE_H, TOTAL_PITCHES, pitchToYEdit } from './coords';
import { getPreviewPitchRange } from './previewLayout';

export function editClipLayoutKey(trackId: string | null, clipId: string | null): string | null {
	if (!trackId || !clipId) return null;
	return `${trackId}:${clipId}`;
}

/** Scroll vertical pour afficher la plage de hauteurs (même marge ±3 que l'aperçu). */
export function pitchRangeScrollTop(
	minPitch: number,
	maxPitch: number,
	viewportHeight: number
): number {
	const contentH = TOTAL_PITCHES * NOTE_H;
	const maxScroll = Math.max(0, contentH - viewportHeight);
	if (viewportHeight <= 0) return 0;

	const topY = pitchToYEdit(maxPitch);
	const bottomY = pitchToYEdit(minPitch) + NOTE_H;
	const rangeH = bottomY - topY;

	const scrollTop = topY - (viewportHeight - rangeH) / 2;
	return Math.max(0, Math.min(maxScroll, scrollTop));
}

/** Cadrage initial édition : début du clip à gauche, plage de notes visible verticalement. */
export function fitEditScrollForNotes(
	noteList: MidiNote[],
	viewportHeight: number
): { scrollLeft: number; scrollTop: number } {
	const { min, max } = getPreviewPitchRange(noteList);
	return {
		scrollLeft: 0,
		scrollTop: pitchRangeScrollTop(min, max, viewportHeight)
	};
}
