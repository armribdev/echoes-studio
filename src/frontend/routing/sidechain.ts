import type { TrackEffect } from '../../types/global';

interface SidechainTrack {
	id: string;
	name: string;
	effects: TrackEffect[];
}

/** Track → tracks that sidechain from it, from every plugin effect's `sidechain_source`. */
function sidechainFeeds(tracks: SidechainTrack[]): Map<string, string[]> {
	const feeds = new Map<string, string[]>();
	for (const track of tracks) {
		for (const effect of track.effects) {
			if (effect.params.kind !== 'plugin') continue;
			const source = effect.params.params.sidechain_source;
			if (!source) continue;
			feeds.set(source, [...(feeds.get(source) ?? []), track.id]);
		}
	}
	return feeds;
}

function reaches(feeds: Map<string, string[]>, from: string, to: string): boolean {
	const stack = [from];
	const visited = new Set<string>();
	while (stack.length > 0) {
		const current = stack.pop() as string;
		if (current === to) return true;
		if (visited.has(current)) continue;
		visited.add(current);
		stack.push(...(feeds.get(current) ?? []));
	}
	return false;
}

/**
 * Tracks that can feed a sidechain into `targetTrackId`. Excludes the track itself
 * and any track it already feeds: the engine would silently drop such a loop.
 */
export function sidechainSourceCandidates<T extends SidechainTrack>(
	tracks: T[],
	targetTrackId: string
): T[] {
	const feeds = sidechainFeeds(tracks);
	return tracks.filter((track) => !reaches(feeds, targetTrackId, track.id));
}
