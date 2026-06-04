import type { Track } from '../types/global';

export const ARRANGE_TRACK_HEIGHT = 100;
export const ARRANGE_POINTER_DRAG_THRESHOLD = 3;

export function trackIndexFromArrangeY(y: number, trackCount: number): number | null {
	if (trackCount <= 0) return null;
	return Math.min(Math.max(0, Math.floor(y / ARRANGE_TRACK_HEIGHT)), trackCount - 1);
}

/** Track row for arrange hit-testing; `trackCount` means a new row below existing tracks. */
export function arrangeTrackIndexFromY(y: number, trackCount: number): number {
	if (trackCount <= 0) return 0;
	const row = Math.floor(y / ARRANGE_TRACK_HEIGHT);
	if (row >= trackCount) return trackCount;
	return Math.max(0, row);
}

export function isNewArrangeTrackRow(trackIndex: number, trackCount: number): boolean {
	return trackCount === 0 || trackIndex >= trackCount;
}

const ARRANGE_MIN_VISIBLE_BARS = 8;
const ARRANGE_VISIBLE_BARS_AFTER_LAST_CLIP = 4;

function maxClipEndSec(tracks: Track[]): number {
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

/** Dernière mesure visible : max(8, fin du dernier clip en mesures + 4). */
export function arrangeVisibleMaxBars(tracks: Track[], bpm: number, numerator: number): number {
	const barSec = (numerator * 60) / bpm;
	const maxClipEndBar = maxClipEndSec(tracks) / barSec;
	return Math.max(ARRANGE_MIN_VISIBLE_BARS, maxClipEndBar + ARRANGE_VISIBLE_BARS_AFTER_LAST_CLIP);
}

/** px/beat au dézoom max : toute la timeline (max bars) remplit le viewport. */
export function arrangeMinPxPerBeat(
	viewportWidthPx: number,
	maxBars: number,
	numerator: number,
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
	bpm: number,
): number {
	const pxPerBeat = arrangeMinPxPerBeat(viewportWidthPx, maxBars, numerator);
	return (pxPerBeat * bpm) / 60;
}

export function arrangeMinLogScale(
	viewportWidthPx: number,
	maxBars: number,
	numerator: number,
	bpm: number,
): number {
	return Math.log(Math.max(arrangeMinScale(viewportWidthPx, maxBars, numerator, bpm), 1e-9));
}

export type ArrangeSelbox = { x0: number; y0: number; x1: number; y1: number };

export function clipsInArrangeSelbox(
	tracks: Track[],
	selbox: ArrangeSelbox,
	scale: number,
): string[] {
	const leftPx = Math.min(selbox.x0, selbox.x1);
	const rightPx = Math.max(selbox.x0, selbox.x1);
	const topPx = Math.min(selbox.y0, selbox.y1);
	const bottomPx = Math.max(selbox.y0, selbox.y1);
	const timeStart = leftPx / scale;
	const timeEnd = rightPx / scale;
	const trackStart = Math.max(0, Math.floor(topPx / ARRANGE_TRACK_HEIGHT));
	const trackEnd = Math.min(
		tracks.length - 1,
		Math.floor(Math.max(topPx, bottomPx - 1) / ARRANGE_TRACK_HEIGHT),
	);

	const ids: string[] = [];
	for (let ti = trackStart; ti <= trackEnd; ti++) {
		const track = tracks[ti];
		if (!track) continue;
		for (const clip of track.clips) {
			if (clip.start < timeEnd && clip.end > timeStart) ids.push(clip.id);
		}
		for (const clip of track.midi_clips ?? []) {
			if (clip.start < timeEnd && clip.end > timeStart) ids.push(clip.id);
		}
	}
	return ids;
}
