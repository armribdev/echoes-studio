import type { Track } from '../types/global';

export interface TempoInfo {
	bpm: number;
	numerator: number;
}

/**
 * Returns the finest musical subdivision (in seconds) that still spans
 * at least MIN_SNAP_PX pixels at the current zoom level.
 */
export function getSnapSubdivisionSec(
	tempo: TempoInfo,
	scale: number,
	midiGridBeats: number
): number {
	const MIN_SNAP_PX = 8;
	const beatSec = 60 / tempo.bpm;
	const barSec = tempo.numerator * beatSec;
	const projectGridSec = beatSec * midiGridBeats;

	let adaptiveSec = barSec;
	for (const denom of [32, 16, 8, 4, 2, 1]) {
		const subSec = beatSec / denom;
		if (subSec * scale >= MIN_SNAP_PX) {
			adaptiveSec = subSec;
			break;
		}
	}
	if (adaptiveSec === barSec) {
		let multiple = 1;
		while (barSec * multiple * scale < MIN_SNAP_PX) multiple *= 2;
		adaptiveSec = barSec * multiple;
	}

	return Math.max(projectGridSec, adaptiveSec);
}

export function snapPosition(
	positionSec: number,
	altKey: boolean,
	tempo: TempoInfo,
	scale: number,
	midiGridBeats: number,
	tracks: Track[],
	excludeClipId?: string
): number {
	if (altKey) return positionSec;
	const thresholdSec = 10 / scale;

	const candidates: number[] = [];

	const subdivSec = getSnapSubdivisionSec(tempo, scale, midiGridBeats);
	candidates.push(Math.round(positionSec / subdivSec) * subdivSec);

	for (const track of tracks) {
		for (const clip of track.clips) {
			if (clip.id === excludeClipId) continue;
			candidates.push(clip.start);
			candidates.push(clip.end);
		}
		for (const clip of track.midi_clips ?? []) {
			if (clip.id === excludeClipId) continue;
			candidates.push(clip.start);
			candidates.push(clip.end);
		}
	}

	let best = positionSec;
	let bestDist = thresholdSec;
	for (const c of candidates) {
		const d = Math.abs(c - positionSec);
		if (d < bestDist) {
			bestDist = d;
			best = c;
		}
	}
	return best;
}
