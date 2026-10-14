import { describe, expect, it } from 'vitest';
import type { TrackEffect } from '../../types/global';
import { sidechainSourceCandidates } from './sidechain';

function pluginEffect(sidechainSource: string | null): TrackEffect {
	return {
		id: `fx-${sidechainSource ?? 'none'}`,
		enabled: true,
		params: {
			kind: 'plugin',
			params: {
				plugin_id: 'clap:comp',
				name: 'Comp',
				format: 'CLAP',
				params: [],
				sandboxed: false,
				sidechain_source: sidechainSource
			}
		}
	};
}

function track(id: string, sidechainSource: string | null = null) {
	return { id, name: id, effects: sidechainSource === null ? [] : [pluginEffect(sidechainSource)] };
}

const ids = (tracks: { id: string }[]) => tracks.map((t) => t.id);

describe('sidechainSourceCandidates', () => {
	it('offers every other track when nothing is routed', () => {
		const tracks = [track('a'), track('b'), track('c')];
		expect(ids(sidechainSourceCandidates(tracks, 'a'))).toEqual(['b', 'c']);
	});

	it('excludes the track itself', () => {
		const tracks = [track('a'), track('b')];
		expect(ids(sidechainSourceCandidates(tracks, 'b'))).not.toContain('b');
	});

	it('excludes a track that already sidechains from the target (direct loop)', () => {
		const tracks = [track('a'), track('b', 'a')];
		expect(ids(sidechainSourceCandidates(tracks, 'a'))).toEqual([]);
	});

	it('excludes a track reached through a chain of sidechains', () => {
		const tracks = [track('a'), track('b', 'a'), track('c', 'b')];
		expect(ids(sidechainSourceCandidates(tracks, 'a'))).toEqual([]);
		expect(ids(sidechainSourceCandidates(tracks, 'b'))).toEqual(['a']);
	});

	it('keeps tracks that are unrelated to the target', () => {
		const tracks = [track('a'), track('b', 'a'), track('c')];
		expect(ids(sidechainSourceCandidates(tracks, 'a'))).toEqual(['c']);
	});
});
