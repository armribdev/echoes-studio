import { describe, expect, it } from 'vitest';
import {
	canDropAssetOnLane,
	defaultSourceForContext,
	laneFocusForTrack,
	openContextFromLaneFocus,
	sourceForContext
} from './laneFocus';
import { createCollection } from './collections';

describe('laneFocusForTrack', () => {
	it('classifies empty, audio-only, midi-only, and mixed tracks', () => {
		expect(laneFocusForTrack(null)).toBe('neutral');
		expect(laneFocusForTrack({ clips: [], midi_clips: [] })).toBe('neutral');
		expect(laneFocusForTrack({ clips: [{}], midi_clips: [] })).toBe('audio');
		expect(laneFocusForTrack({ clips: [], midi_clips: [{}] })).toBe('midi');
		expect(laneFocusForTrack({ clips: [{}], midi_clips: [{}] })).toBe('neutral');
	});
});

describe('openContextFromLaneFocus / defaultSourceForContext', () => {
	it('maps lane focus to open context and default library source', () => {
		expect(openContextFromLaneFocus('audio', true)).toBe('track_audio_lane');
		expect(openContextFromLaneFocus('midi', true)).toBe('track_midi_lane');
		expect(openContextFromLaneFocus('neutral', true)).toBe('track_empty');
		expect(openContextFromLaneFocus('audio', false)).toBe('track_empty');

		expect(defaultSourceForContext('track_midi_lane')).toBe('chords');
		expect(defaultSourceForContext('track_audio_lane')).toBe('audio');
		expect(defaultSourceForContext('track_empty')).toBe('audio');
	});
});

describe('sourceForContext', () => {
	const collection = createCollection('Techno', 'techno', null);
	const collectionSource = `collection:${collection.id}` as const;
	const none = { collections: [], indexedFolders: [] };

	it('uses the factory default until the user chooses', () => {
		expect(sourceForContext('track_midi_lane', {}, none)).toBe('chords');
	});

	it('uses the source remembered for that context only', () => {
		const remembered = { track_audio_lane: 'drums' } as const;
		expect(sourceForContext('track_audio_lane', remembered, none)).toBe('drums');
		expect(sourceForContext('track_empty', remembered, none)).toBe('audio');
	});

	it('follows a remembered collection while it exists', () => {
		const remembered = { track_empty: collectionSource } as const;
		const known = { collections: [collection], indexedFolders: [] };
		expect(sourceForContext('track_empty', remembered, known)).toBe(collectionSource);
		expect(sourceForContext('track_empty', remembered, none)).toBe('audio');
	});

	it('follows a remembered pack while its folder is indexed', () => {
		const remembered = { track_empty: 'pack:C:\\Kits' } as const;
		const known = { collections: [], indexedFolders: ['C:\\Kits'] };
		expect(sourceForContext('track_empty', remembered, known)).toBe('pack:C:\\Kits');
		expect(sourceForContext('track_empty', remembered, none)).toBe('audio');
	});
});

describe('canDropAssetOnLane', () => {
	it('rejects cross-type drops on focused lanes', () => {
		expect(canDropAssetOnLane('one_shot', 'neutral')).toBe(true);
		expect(canDropAssetOnLane('midi_pattern', 'neutral')).toBe(true);
		expect(canDropAssetOnLane('loop', 'audio')).toBe(true);
		expect(canDropAssetOnLane('chord_progression', 'audio')).toBe(false);
		expect(canDropAssetOnLane('midi_pattern', 'midi')).toBe(true);
		expect(canDropAssetOnLane('one_shot', 'midi')).toBe(false);
	});
});
