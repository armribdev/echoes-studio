import type { Track } from '../types/global';

export interface TempoInfo {
	bpm: number;
	numerator: number;
}

/**
 * Returns the finest musical subdivision (in seconds) that still spans
 * at least MIN_SNAP_PX pixels at the current zoom level.
 */
export function getSnapSubdivisionSec(tempo: TempoInfo, scale: number): number {
	const MIN_SNAP_PX = 8;
	const beatSec = 60 / tempo.bpm;
	const barSec = tempo.numerator * beatSec;

	for (const denom of [32, 16, 8, 4, 2, 1]) {
		const subSec = beatSec / denom;
		if (subSec * scale >= MIN_SNAP_PX) return subSec;
	}

	let multiple = 1;
	while (barSec * multiple * scale < MIN_SNAP_PX) multiple *= 2;
	return barSec * multiple;
}

export function snapPosition(
	positionSec: number,
	altKey: boolean,
	tempo: TempoInfo,
	scale: number,
	tracks: Track[],
	excludeClip?: { trackIndex: number; clipIndex: number }
): number {
	if (altKey) return positionSec;
	const thresholdSec = 10 / scale;

	const candidates: number[] = [];

	const subdivSec = getSnapSubdivisionSec(tempo, scale);
	candidates.push(Math.round(positionSec / subdivSec) * subdivSec);

	for (let ti = 0; ti < tracks.length; ti++) {
		const clips = tracks[ti].clips;
		for (let ci = 0; ci < clips.length; ci++) {
			if (excludeClip && ti === excludeClip.trackIndex && ci === excludeClip.clipIndex) continue;
			candidates.push(clips[ci].start);
			candidates.push(clips[ci].end);
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
