import { describe, expect, it } from 'vitest';
import type { MidiClip } from '../../types/global';
import { linkedMidiClipIds, midiContentOf } from './index';

describe('midiContentOf', () => {
	const content = { notes: [], channel_controls: [] };

	it('returns the content the clip references', () => {
		expect(midiContentOf({ midi_contents: { c1: content } }, { content_id: 'c1' })).toBe(content);
	});

	it('returns the same empty content when the pool has no entry', () => {
		const state = { midi_contents: {} };
		const first = midiContentOf(state, { content_id: 'missing' });
		expect(first).toEqual({ notes: [], channel_controls: [] });
		expect(midiContentOf(state, { content_id: 'other' })).toBe(first);
	});
});

describe('linkedMidiClipIds', () => {
	function clip(id: string, contentId: string): MidiClip {
		return { id, start: 0, end: 1, offset: 0, content_id: contentId };
	}

	function track(...midiClips: MidiClip[]) {
		return { midi_clips: midiClips };
	}

	it('is empty when every clip has its own content', () => {
		const tracks = [track(clip('a', 'c1'), clip('b', 'c2'))];
		expect(linkedMidiClipIds({ tracks }).size).toBe(0);
	});

	it('flags every clip that shares a content, across tracks', () => {
		const tracks = [
			track(clip('a', 'c1'), clip('solo', 'c2')),
			track(clip('b', 'c1'), clip('c', 'c1'))
		];
		expect([...linkedMidiClipIds({ tracks })].sort()).toEqual(['a', 'b', 'c']);
	});

	it('stops flagging the last clip once its sibling is gone', () => {
		const tracks = [track(clip('a', 'c1'))];
		expect(linkedMidiClipIds({ tracks }).size).toBe(0);
	});
});
