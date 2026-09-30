import type { ArrangementMarker, ArrangementRegion } from '../../types/global';

export type LocatorKind = 'marker' | 'region';

/** Timeline locator ordered by musical time (marker position / region start). */
export interface LocatorEntry {
	kind: LocatorKind;
	id: string;
	beat: number;
	name: string;
}

/** Default new region length: 4 bars, in quarter-note beats. */
export function defaultRegionLengthBeats(numerator: number): number {
	return 4 * Math.max(1, numerator);
}

/** Region span of 4 bars starting at the bar that contains `beat`. */
export function defaultRegionSpanAt(
	beat: number,
	numerator: number
): { start: number; end: number } {
	const barBeats = Math.max(1, numerator);
	const start = Math.max(0, Math.floor(beat / barBeats) * barBeats);
	return { start, end: start + defaultRegionLengthBeats(barBeats) };
}

export function collectLocators(
	markers: ArrangementMarker[],
	regions: ArrangementRegion[]
): LocatorEntry[] {
	const entries: LocatorEntry[] = [
		...markers.map((m) => ({
			kind: 'marker' as const,
			id: m.id,
			beat: m.position_beats,
			name: m.name
		})),
		...regions.map((r) => ({
			kind: 'region' as const,
			id: r.id,
			beat: r.start_beats,
			name: r.name
		}))
	];
	entries.sort((a, b) => a.beat - b.beat || a.id.localeCompare(b.id));
	return entries;
}

/** Next/previous locator relative to the playhead (wraps). */
export function navigateLocator(
	entries: LocatorEntry[],
	_current: { kind: LocatorKind; id: string } | null,
	playheadBeat: number,
	direction: -1 | 1
): LocatorEntry | null {
	if (entries.length === 0) return null;

	if (direction > 0) {
		return entries.find((e) => e.beat > playheadBeat + 1e-9) ?? entries[0];
	}
	for (let i = entries.length - 1; i >= 0; i--) {
		if (entries[i].beat < playheadBeat - 1e-9) return entries[i];
	}
	return entries[entries.length - 1];
}
