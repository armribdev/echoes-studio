import type { Track } from '../types/global';

export const ARRANGE_TRACK_HEIGHT = 100;
export const ARRANGE_LANE_HEIGHT = 56;
export const ARRANGE_POINTER_DRAG_THRESHOLD = 3;

/** Left accroché aux pixels device ; largeur = durée exacte (évite le crawl des notes au resize). */
export function arrangeClipFrame(
	startSec: number,
	endSec: number,
	scale: number,
	dpr = typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1
): { leftPx: number; widthPx: number } {
	const leftPx = Math.round(startSec * scale * dpr) / dpr;
	const widthPx = Math.max(1 / dpr, (endSec - startSec) * scale);
	return { leftPx, widthPx };
}

/** A track row is the track itself plus its automation lanes. */
export function arrangeRowHeight(track: Track): number {
	return ARRANGE_TRACK_HEIGHT + (track.automation_lanes?.length ?? 0) * ARRANGE_LANE_HEIGHT;
}

/** Top of each track row, followed by the total height. */
export function arrangeRowTops(tracks: Track[]): number[] {
	const tops = [0];
	for (const track of tracks) tops.push(tops[tops.length - 1] + arrangeRowHeight(track));
	return tops;
}

function rowIndexFromY(y: number, tracks: Track[]): number {
	let bottom = 0;
	for (let index = 0; index < tracks.length; index++) {
		bottom += arrangeRowHeight(tracks[index]);
		if (y < bottom) return index;
	}
	return tracks.length;
}

export function trackIndexFromArrangeY(y: number, tracks: Track[]): number | null {
	if (tracks.length === 0) return null;
	return Math.min(rowIndexFromY(y, tracks), tracks.length - 1);
}

/** Track row for arrange hit-testing; `tracks.length` means a new row below existing tracks. */
export function arrangeTrackIndexFromY(y: number, tracks: Track[]): number {
	return rowIndexFromY(y, tracks);
}

export function isNewArrangeTrackRow(trackIndex: number, trackCount: number): boolean {
	return trackCount === 0 || trackIndex >= trackCount;
}

const ARRANGE_MIN_VISIBLE_BARS = 8;
const ARRANGE_VISIBLE_BARS_AFTER_LAST_CLIP = 4;

/** Fin temporelle du contenu arrange (clips audio + MIDI). */
export function maxClipEndSec(tracks: Track[]): number {
	let maxEnd = 0;
	for (const track of tracks) {
		for (const clip of track.clips) {
			if (clip.end > maxEnd) maxEnd = clip.end;
		}
		for (const clip of track.midi_clips ?? []) {
			if (clip.end > maxEnd) maxEnd = clip.end;
		}
	}
	return maxEnd;
}

/** Dernière mesure visible : max(8, ceil(fin clip en mesures) + 4). Ceil évite le thrash de largeur pendant un drag du dernier clip. */
export function arrangeVisibleMaxBars(
	tracks: Track[],
	bpm: number,
	numerator: number,
	extraEndsSec: Iterable<number> = []
): number {
	let maxEnd = maxClipEndSec(tracks);
	for (const end of extraEndsSec) {
		if (end > maxEnd) maxEnd = end;
	}
	const barSec = (numerator * 60) / bpm;
	const maxClipEndBar = maxEnd / barSec;
	return Math.max(
		ARRANGE_MIN_VISIBLE_BARS,
		Math.ceil(maxClipEndBar) + ARRANGE_VISIBLE_BARS_AFTER_LAST_CLIP
	);
}

/**
 * Étend le viewport pour contenir `[rangeStartSec, rangeEndSec]`.
 * - dépasse à droite → la limite droite avance (gauche fixe)
 * - dépasse à gauche → la limite gauche recule (droite fixe)
 * - les deux → les deux bougent
 * Ne zoome jamais vers l'avant. Retourne null si déjà contenu.
 */
export function arrangeViewportExpandToRange(opts: {
	rangeStartSec: number;
	rangeEndSec: number;
	viewStartSec: number;
	viewEndSec: number;
	viewportWidthPx: number;
	currentScale: number;
	minScale: number;
}): { scale: number; scrollLeft: number; viewStartSec: number; viewEndSec: number } | null {
	const {
		rangeStartSec,
		rangeEndSec,
		viewStartSec,
		viewEndSec,
		viewportWidthPx,
		currentScale,
		minScale
	} = opts;

	if (viewportWidthPx <= 0 || currentScale <= 0) return null;
	const rangeStart = Math.min(rangeStartSec, rangeEndSec);
	const rangeEnd = Math.max(rangeStartSec, rangeEndSec);
	if (rangeEnd - rangeStart < 1e-9) return null;

	const epsSec = 2 / currentScale;
	const overflowLeft = rangeStart < viewStartSec - epsSec;
	const overflowRight = rangeEnd > viewEndSec + epsSec;
	if (!overflowLeft && !overflowRight) return null;

	const nextStart = overflowLeft ? rangeStart : viewStartSec;
	const nextEnd = overflowRight ? rangeEnd : viewEndSec;
	const spanSec = Math.max(nextEnd - nextStart, 1e-9);
	let scale = viewportWidthPx / spanSec;
	if (scale >= currentScale - 1e-12) {
		// Même span (cas limite) : pan éventuel uniquement si hors vue totale.
		return null;
	}

	scale = Math.max(minScale, scale);
	let scrollLeft = Math.max(0, nextStart * scale);

	if (scrollLeft + viewportWidthPx < nextEnd * scale - 1e-6) {
		scale = minScale;
		scrollLeft = 0;
	}

	const fittedStart = scrollLeft / scale;
	const fittedEnd = (scrollLeft + viewportWidthPx) / scale;
	return { scale, scrollLeft, viewStartSec: fittedStart, viewEndSec: fittedEnd };
}

/**
 * Après un remove qui raccourcit la timeline : resserre la vue (zoom avant)
 * si l'ancienne fin était visible / la vue débordait au-delà de la nouvelle fin.
 * Ancré à gauche ; retourne null si rien à changer.
 */
export function arrangeViewportContractAfterShrink(opts: {
	newTimelineEndSec: number;
	prevTimelineEndSec: number;
	viewStartSec: number;
	viewEndSec: number;
	viewportWidthPx: number;
	currentScale: number;
	minScale: number;
	maxScale: number;
}): { scale: number; scrollLeft: number } | null {
	const {
		newTimelineEndSec,
		prevTimelineEndSec,
		viewStartSec,
		viewEndSec,
		viewportWidthPx,
		currentScale,
		minScale,
		maxScale
	} = opts;

	if (viewportWidthPx <= 0 || currentScale <= 0) return null;
	if (newTimelineEndSec >= prevTimelineEndSec - 1e-9) return null;

	const epsSec = 2 / currentScale;
	const oldEndWasVisible = viewEndSec >= prevTimelineEndSec - epsSec;
	const viewPastNewEnd = viewEndSec > newTimelineEndSec + epsSec;
	const belowNewFloor = currentScale < minScale - 1e-9;

	if (!oldEndWasVisible && !viewPastNewEnd && !belowNewFloor) {
		const maxScroll = Math.max(0, newTimelineEndSec * currentScale - viewportWidthPx);
		if (viewStartSec * currentScale <= maxScroll + 0.5) return null;
		return {
			scale: currentScale,
			scrollLeft: maxScroll
		};
	}

	const targetEnd = Math.max(newTimelineEndSec, 1e-6);
	let left = Math.min(viewStartSec, targetEnd);
	if (left < 0) left = 0;

	// Si on était au dézoom max (vue ≈ toute la timeline), recentrer sur [0, newEnd].
	const wasFullSpan =
		viewStartSec <= epsSec && Math.abs(viewEndSec - prevTimelineEndSec) <= epsSec * 4;
	if (wasFullSpan || belowNewFloor) {
		left = 0;
	}

	const spanSec = Math.max(targetEnd - left, 1e-9);
	let scale = viewportWidthPx / spanSec;
	scale = Math.min(maxScale, Math.max(minScale, scale));
	// Uniquement resserrer (zoom avant) ou pan — pas dézoomer davantage.
	if (scale < currentScale - 1e-12 && !belowNewFloor) {
		scale = currentScale;
	}

	let scrollLeft = Math.max(0, left * scale);
	const maxScroll = Math.max(0, targetEnd * scale - viewportWidthPx);
	scrollLeft = Math.min(scrollLeft, maxScroll);

	if (
		Math.abs(scale - currentScale) < 1e-4 &&
		Math.abs(scrollLeft - viewStartSec * currentScale) < 0.5
	) {
		return null;
	}

	return { scale, scrollLeft };
}

export function easeOutCubic(t: number): number {
	const x = Math.min(1, Math.max(0, t));
	return 1 - (1 - x) ** 3;
}

/**
 * Pendant un drag/resize, ne jamais rétrécir l'étendue visible.
 * Sinon, franchir une mesure fait chuter maxBars d'un coup → scrollWidth,
 * clamp de scrollLeft, et parfois enforceArrangeMinZoom → sauts sous le curseur.
 */
export function arrangeMaxBarsHeld(liveMaxBars: number, holdMaxBars: number | null): number {
	return holdMaxBars == null ? liveMaxBars : Math.max(liveMaxBars, holdMaxBars);
}

/** px/beat au dézoom max : toute la timeline (max bars) remplit le viewport. */
export function arrangeMinPxPerBeat(
	viewportWidthPx: number,
	maxBars: number,
	numerator: number
): number {
	const totalBeats = maxBars * numerator;
	if (viewportWidthPx <= 0 || totalBeats <= 0) return 0;
	return viewportWidthPx / totalBeats;
}

/** Échelle px/s au dézoom max (Ctrl+molette et scrollbar ne peuvent pas aller en dessous). */
export function arrangeMinScale(
	viewportWidthPx: number,
	maxBars: number,
	numerator: number,
	bpm: number
): number {
	const pxPerBeat = arrangeMinPxPerBeat(viewportWidthPx, maxBars, numerator);
	return (pxPerBeat * bpm) / 60;
}

export function arrangeMinLogScale(
	viewportWidthPx: number,
	maxBars: number,
	numerator: number,
	bpm: number
): number {
	return Math.log(Math.max(arrangeMinScale(viewportWidthPx, maxBars, numerator, bpm), 1e-9));
}

export type ArrangeSelbox = { x0: number; y0: number; x1: number; y1: number };

export function clipsInArrangeSelbox(
	tracks: Track[],
	selbox: ArrangeSelbox,
	scale: number,
	pendings: {
		id: string;
		trackId: string;
		start: number;
		end: number;
		engineClipId?: string;
	}[] = []
): string[] {
	const leftPx = Math.min(selbox.x0, selbox.x1);
	const rightPx = Math.max(selbox.x0, selbox.x1);
	const topPx = Math.min(selbox.y0, selbox.y1);
	const bottomPx = Math.max(selbox.y0, selbox.y1);
	const timeStart = leftPx / scale;
	const timeEnd = rightPx / scale;
	const trackStart = rowIndexFromY(topPx, tracks);
	const trackEnd = Math.min(
		tracks.length - 1,
		rowIndexFromY(Math.max(topPx, bottomPx - 1), tracks)
	);

	const ids: string[] = [];
	const seen = new Set<string>();
	const pushId = (id: string) => {
		if (seen.has(id)) return;
		seen.add(id);
		ids.push(id);
	};

	for (let ti = trackStart; ti <= trackEnd; ti++) {
		const track = tracks[ti];
		if (!track) continue;
		for (const clip of track.clips) {
			if (clip.start < timeEnd && clip.end > timeStart) pushId(clip.id);
		}
		for (const clip of track.midi_clips ?? []) {
			if (clip.start < timeEnd && clip.end > timeStart) pushId(clip.id);
		}
		for (const pending of pendings) {
			if (pending.trackId !== track.id) continue;
			if (pending.start < timeEnd && pending.end > timeStart) {
				pushId(pending.engineClipId ?? pending.id);
			}
		}
	}
	return ids;
}
