import { describe, expect, it } from 'vitest';
import { ev, type EngineEvent } from '../protocol/events';
import type {
	Clip,
	MidiClip,
	MidiContent,
	MidiNote,
	Project,
	SamplerPad
} from '../../types/global';
import { SAMPLER_INSTRUMENT_ID } from './sampler';
import {
	applyEvent,
	emptyProjectState,
	projectFromSnapshot,
	replayEvents,
	type ProjectState
} from './reducer';

function event(type: string, payload: unknown, id = 'evt'): EngineEvent {
	return { id, projectId: 'proj', type, timestamp: 0, payload };
}

function audioClip(id: string, start = 0, end = 1): Clip {
	return {
		id,
		start,
		end,
		offset: 0,
		file_path: `/audio/${id}.wav`,
		sample_rate: 48000,
		peaks_64: [],
		peaks_512: [],
		peaks_4096: []
	};
}

function midiClip(id: string, start = 0, end = 2, contentId = `content-${id}`): MidiClip {
	return { id, start, end, offset: 0, content_id: contentId };
}

function midiContent(notes: MidiNote[] = []): MidiContent {
	return { notes, channel_controls: [] };
}

function note(id: string, pitch = 60): MidiNote {
	return { id, pitch, start: 0, duration: 1, velocity: 100 };
}

describe('emptyProjectState', () => {
	it('starts with defaults and no tracks', () => {
		const state = emptyProjectState();
		expect(state.name).toBe('Sans titre');
		expect(state.bpm).toBe(120);
		expect(state.tracks).toEqual([]);
		expect(state.markers).toEqual([]);
		expect(state.regions).toEqual([]);
	});
});

describe('replayEvents — project integration stream', () => {
	it('rebuilds arrangement state from a track/clip/marker/region stream', () => {
		const clip = audioClip('c1', 0, 2);
		const state = replayEvents([
			event(ev.TRACK_CREATED, { id: 't1', name: 'Drums' }),
			event(ev.TRACK_CREATED, { id: 't2', name: 'Bass', instrument: 'PolySynth' }),
			event(ev.CLIP_ADDED, { track_id: 't1', clip }),
			event(ev.TRACK_UPDATED, { id: 't1', volume: 64, pan: -0.2, color: '#ff0000' }),
			event(ev.TRACK_MUTED, { id: 't2', muted: true }),
			event(ev.TEMPO_UPDATED, {
				bpm: 100,
				numerator: 3,
				denominator: 4,
				midi_grid_beats: 0.5
			}),
			event(ev.MARKER_ADDED, {
				marker: { id: 'm1', name: 'Intro', position_beats: 0 }
			}),
			event(ev.REGION_ADDED, {
				region: { id: 'r1', name: 'Verse', start_beats: 16, end_beats: 32 }
			}),
			event(ev.PROJECT_NAME_UPDATED, { name: 'Demo' })
		]);

		expect(state.name).toBe('Demo');
		expect(state.bpm).toBe(100);
		expect(state.numerator).toBe(3);
		expect(state.midi_grid_beats).toBe(0.5);
		expect(state.tracks).toHaveLength(2);
		expect(state.tracks[0]).toMatchObject({
			id: 't1',
			name: 'Drums',
			volume: 64,
			pan: -0.2,
			color: '#ff0000'
		});
		expect(state.tracks[0].clips[0]).toMatchObject({ id: 'c1', trackId: 't1', start: 0, end: 2 });
		expect(state.tracks[1]).toMatchObject({
			id: 't2',
			instrument: 'PolySynth',
			muted: true
		});
		expect(state.markers).toEqual([{ id: 'm1', name: 'Intro', position_beats: 0 }]);
		expect(state.regions).toEqual([{ id: 'r1', name: 'Verse', start_beats: 16, end_beats: 32 }]);
	});

	it('moves clips between tracks and removes them', () => {
		const clip = audioClip('c1', 1, 3);
		let state = replayEvents([
			event(ev.TRACK_CREATED, { id: 't1', name: 'A' }),
			event(ev.TRACK_CREATED, { id: 't2', name: 'B' }),
			event(ev.CLIP_ADDED, { track_id: 't1', clip }),
			event(ev.CLIP_MOVED, { track_id: 't1', clip_id: 'c1', start: 4, end: 6, offset: 0.5 })
		]);
		expect(state.tracks[0].clips[0]).toMatchObject({ start: 4, end: 6, offset: 0.5 });

		state = applyEvent(
			state,
			event(ev.CLIP_MOVED_TO_TRACK, {
				from_track_id: 't1',
				to_track_id: 't2',
				clip: { ...clip, start: 4, end: 6, offset: 0.5 }
			})
		);
		expect(state.tracks[0].clips).toHaveLength(0);
		expect(state.tracks[1].clips[0]).toMatchObject({ id: 'c1', trackId: 't2' });

		state = applyEvent(state, event(ev.CLIP_REMOVED, { track_id: 't2', clip_id: 'c1' }));
		expect(state.tracks[1].clips).toHaveLength(0);
	});

	it('is idempotent for TRACK_CREATED and CLIP_ADDED', () => {
		const clip = audioClip('c1');
		const once = replayEvents([
			event(ev.TRACK_CREATED, { id: 't1', name: 'A' }),
			event(ev.CLIP_ADDED, { track_id: 't1', clip })
		]);
		const twice = replayEvents(
			[
				event(ev.TRACK_CREATED, { id: 't1', name: 'Ignored' }),
				event(ev.CLIP_ADDED, { track_id: 't1', clip })
			],
			once
		);
		expect(twice.tracks).toHaveLength(1);
		expect(twice.tracks[0].name).toBe('A');
		expect(twice.tracks[0].clips).toHaveLength(1);
	});

	it('enforces exclusive solo across tracks', () => {
		const state = replayEvents([
			event(ev.TRACK_CREATED, { id: 't1', name: 'A' }),
			event(ev.TRACK_CREATED, { id: 't2', name: 'B' }),
			event(ev.TRACK_SOLOED, { id: 't1', solo: true }),
			event(ev.TRACK_SOLOED, { id: 't2', solo: true })
		]);
		expect(state.tracks.map((t) => t.solo)).toEqual([false, true]);

		const cleared = applyEvent(state, event(ev.TRACK_SOLOED, { id: 't2', solo: false }));
		expect(cleared.tracks.map((t) => t.solo)).toEqual([false, false]);
	});

	it('manages MIDI clips and their content on a track', () => {
		const clip = midiClip('mc1', 0, 4);
		let state = replayEvents([
			event(ev.TRACK_CREATED, { id: 't1', name: 'Keys' }),
			event(ev.MIDI_CLIP_ADDED, {
				track_id: 't1',
				clip,
				content: midiContent([note('n1')])
			}),
			event(ev.MIDI_CONTENT_NOTES_SET, {
				content_id: 'content-mc1',
				notes: [{ id: 'n2', pitch: 64, start: 1, duration: 0.5, velocity: 80 }]
			})
		]);
		expect(state.midi_contents['content-mc1'].notes).toEqual([
			{ id: 'n2', pitch: 64, start: 1, duration: 0.5, velocity: 80 }
		]);

		state = applyEvent(
			state,
			event(ev.MIDI_CONTENT_CONTROLS_SET, {
				content_id: 'content-mc1',
				channel_controls: [{ start: 1, kind: 'control_change', number: 64, value: 127 }]
			})
		);
		expect(state.midi_contents['content-mc1'].channel_controls).toEqual([
			{ start: 1, kind: 'control_change', number: 64, value: 127 }
		]);
		expect(state.midi_contents['content-mc1'].notes).toHaveLength(1);

		state = applyEvent(
			state,
			event(ev.MIDI_CLIP_MOVED, {
				track_id: 't1',
				clip_id: 'mc1',
				start: 8,
				end: 12
			})
		);
		expect(state.tracks[0].midi_clips[0]).toMatchObject({ start: 8, end: 12 });

		state = applyEvent(state, event(ev.MIDI_CLIP_REMOVED, { track_id: 't1', clip_id: 'mc1' }));
		expect(state.tracks[0].midi_clips).toHaveLength(0);
		expect(state.midi_contents).toEqual({});
	});

	it('updates markers and regions then removes them', () => {
		let state = replayEvents([
			event(ev.MARKER_ADDED, {
				marker: { id: 'm1', name: 'A', position_beats: 0 }
			}),
			event(ev.REGION_ADDED, {
				region: { id: 'r1', name: 'Loop', start_beats: 0, end_beats: 16 }
			}),
			event(ev.MARKER_UPDATED, {
				marker: { id: 'm1', name: 'Chorus', position_beats: 32, color: '#0f0' }
			}),
			event(ev.REGION_UPDATED, {
				region: { id: 'r1', name: 'Drop', start_beats: 16, end_beats: 48 }
			})
		]);
		expect(state.markers[0]).toMatchObject({ name: 'Chorus', position_beats: 32 });
		expect(state.regions[0]).toMatchObject({ name: 'Drop', start_beats: 16, end_beats: 48 });

		state = applyEvent(state, event(ev.MARKER_REMOVED, { id: 'm1' }));
		state = applyEvent(state, event(ev.REGION_REMOVED, { id: 'r1' }));
		expect(state.markers).toHaveLength(0);
		expect(state.regions).toHaveLength(0);
	});

	it('adds and removes automation lanes and upserts clips', () => {
		const lane = {
			id: 'l1',
			target: { kind: 'track_gain' as const, track_id: 't1' },
			clips: [] as { id: string; start_s: number; end_s: number; points: never[] }[],
			interpolation: 'linear' as const
		};
		let state = replayEvents([
			event(ev.TRACK_CREATED, { id: 't1', name: 'A' }),
			event(ev.AUTOMATION_LANE_ADDED, { track_id: 't1', lane }),
			event(ev.AUTOMATION_CLIP_UPDATED, {
				track_id: 't1',
				lane_id: 'l1',
				clip: {
					id: 'ac1',
					start_s: 0,
					end_s: 4,
					points: [
						{ time_s: 0, value: 0 },
						{ time_s: 4, value: 1 }
					]
				}
			})
		]);
		expect(state.tracks[0].automation_lanes[0].clips).toHaveLength(1);
		expect(state.tracks[0].automation_lanes[0].clips[0].points).toHaveLength(2);

		state = applyEvent(
			state,
			event(ev.AUTOMATION_CLIP_UPDATED, {
				track_id: 't1',
				lane_id: 'l1',
				clip: {
					id: 'ac1',
					start_s: 0,
					end_s: 2,
					points: [{ time_s: 0, value: 0.5 }]
				}
			})
		);
		expect(state.tracks[0].automation_lanes[0].clips).toHaveLength(1);
		expect(state.tracks[0].automation_lanes[0].clips[0].points).toEqual([
			{ time_s: 0, value: 0.5 }
		]);

		state = applyEvent(
			state,
			event(ev.AUTOMATION_LANE_UPDATED, { track_id: 't1', lane_id: 'l1', interpolation: 'step' })
		);
		expect(state.tracks[0].automation_lanes[0].interpolation).toBe('step');

		state = applyEvent(state, event(ev.AUTOMATION_LANE_REMOVED, { track_id: 't1', lane_id: 'l1' }));
		expect(state.tracks[0].automation_lanes).toHaveLength(0);
	});

	it('ignores unknown events for forward compatibility', () => {
		const base = emptyProjectState();
		const next = applyEvent(base, event('FUTURE_EVENT_V99', { x: 1 }));
		expect(next).toEqual(base);
	});

	it('PROJECT_RESET replaces state from a full snapshot', () => {
		const seeded = replayEvents([
			event(ev.TRACK_CREATED, { id: 'old', name: 'Old' }),
			event(ev.PROJECT_NAME_UPDATED, { name: 'Before' })
		]);
		const dto = {
			name: 'After undo',
			bpm: 90,
			numerator: 4,
			denominator: 4,
			tracks: [
				{
					id: 't1',
					name: 'Restored',
					volume: 70,
					clips: [audioClip('c1')]
				}
			]
		} as Project;

		const reset = applyEvent(seeded, event(ev.PROJECT_RESET, dto));
		expect(reset.name).toBe('After undo');
		expect(reset.bpm).toBe(90);
		expect(reset.tracks).toHaveLength(1);
		expect(reset.tracks[0].clips[0].trackId).toBe('t1');
	});
});

describe('projectFromSnapshot', () => {
	it('fills defaults for optional musical fields', () => {
		const state: ProjectState = projectFromSnapshot({
			name: 'Snap',
			bpm: 128,
			numerator: 4,
			denominator: 4,
			tracks: []
		} as unknown as Project);
		expect(state.midi_grid_beats).toBe(0.25);
		expect(state.key_root).toBe('C');
		expect(state.markers).toEqual([]);
		expect(state.regions).toEqual([]);
		expect(state.midi_contents).toEqual({});
	});

	it('seeds the MIDI content pool from the DTO', () => {
		const content = midiContent([note('n1')]);
		const state = projectFromSnapshot({
			name: 'Snap',
			bpm: 120,
			numerator: 4,
			denominator: 4,
			tracks: [{ id: 't1', name: 'Keys', volume: 80, clips: [], midi_clips: [midiClip('mc1')] }],
			midi_contents: { 'content-mc1': content }
		} as unknown as Project);
		expect(state.midi_contents).toEqual({ 'content-mc1': content });
		expect(state.tracks[0].midi_clips[0].content_id).toBe('content-mc1');
	});
});

describe('MIDI content pool (ADR-032)', () => {
	function withTracks(...trackIds: string[]): ProjectState {
		return replayEvents(trackIds.map((id) => event(ev.TRACK_CREATED, { id, name: id })));
	}

	function addClip(
		state: ProjectState,
		trackId: string,
		clipId: string,
		notes: MidiNote[] = []
	): ProjectState {
		return applyEvent(
			state,
			event(ev.MIDI_CLIP_ADDED, {
				track_id: trackId,
				clip: midiClip(clipId),
				content: midiContent(notes)
			})
		);
	}

	it('creates a clip together with its content', () => {
		const state = addClip(withTracks('t1'), 't1', 'mc1', [note('n1')]);
		expect(state.tracks[0].midi_clips).toEqual([midiClip('mc1')]);
		expect(state.midi_contents).toEqual({ 'content-mc1': midiContent([note('n1')]) });
	});

	it('ignores a replayed MIDI_CLIP_ADDED and keeps later edits', () => {
		const added = event(ev.MIDI_CLIP_ADDED, {
			track_id: 't1',
			clip: midiClip('mc1'),
			content: midiContent([note('n1')])
		});
		const edited = event(ev.MIDI_CONTENT_NOTES_SET, {
			content_id: 'content-mc1',
			notes: [note('n2', 72)]
		});
		const state = replayEvents([added, edited, added], withTracks('t1'));
		expect(state.tracks[0].midi_clips).toHaveLength(1);
		expect(state.midi_contents['content-mc1'].notes).toEqual([note('n2', 72)]);
	});

	it('does not store a content for a clip added to an unknown track', () => {
		const before = withTracks('t1');
		expect(addClip(before, 'missing', 'mc1')).toBe(before);
	});

	it('edits only the targeted content', () => {
		let state = addClip(addClip(withTracks('t1'), 't1', 'a', [note('n1')]), 't1', 'b', [
			note('n2')
		]);
		const untouched = state.midi_contents['content-b'];
		state = applyEvent(
			state,
			event(ev.MIDI_CONTENT_NOTES_SET, { content_id: 'content-a', notes: [note('n3', 48)] })
		);
		expect(state.midi_contents['content-a'].notes).toEqual([note('n3', 48)]);
		expect(state.midi_contents['content-b']).toBe(untouched);
	});

	it('ignores an edit of a content that does not exist', () => {
		const state = addClip(withTracks('t1'), 't1', 'mc1');
		const notes = event(ev.MIDI_CONTENT_NOTES_SET, { content_id: 'ghost', notes: [note('n1')] });
		const controls = event(ev.MIDI_CONTENT_CONTROLS_SET, {
			content_id: 'ghost',
			channel_controls: []
		});
		expect(applyEvent(state, notes)).toBe(state);
		expect(applyEvent(state, controls)).toBe(state);
	});

	it('prunes a content when its last clip is removed', () => {
		let state = addClip(addClip(withTracks('t1'), 't1', 'a'), 't1', 'b');
		state = applyEvent(state, event(ev.MIDI_CLIP_REMOVED, { track_id: 't1', clip_id: 'a' }));
		expect(Object.keys(state.midi_contents)).toEqual(['content-b']);
	});

	it('keeps a content that another clip still references', () => {
		let state = addClip(withTracks('t1', 't2'), 't1', 'a', [note('n1')]);
		state = applyEvent(
			state,
			event(ev.MIDI_CLIP_ADDED, {
				track_id: 't2',
				clip: midiClip('b', 0, 2, 'content-a')
			})
		);
		state = applyEvent(state, event(ev.MIDI_CLIP_REMOVED, { track_id: 't1', clip_id: 'a' }));
		expect(state.midi_contents['content-a'].notes).toEqual([note('n1')]);
		state = applyEvent(state, event(ev.MIDI_CLIP_REMOVED, { track_id: 't2', clip_id: 'b' }));
		expect(state.midi_contents).toEqual({});
	});

	it('prunes the contents of a deleted track only', () => {
		let state = addClip(addClip(withTracks('t1', 't2'), 't1', 'a'), 't2', 'b');
		state = applyEvent(state, event(ev.TRACK_DELETED, { id: 't1' }));
		expect(Object.keys(state.midi_contents)).toEqual(['content-b']);
	});

	it('moves a clip to another track without touching the pool', () => {
		const state = addClip(withTracks('t1', 't2'), 't1', 'a', [note('n1')]);
		const moved = applyEvent(
			state,
			event(ev.MIDI_CLIP_MOVED_TO_TRACK, {
				from_track_id: 't1',
				to_track_id: 't2',
				clip: { ...midiClip('a'), start: 4, end: 6 }
			})
		);
		expect(moved.tracks[0].midi_clips).toEqual([]);
		expect(moved.tracks[1].midi_clips).toEqual([{ ...midiClip('a'), start: 4, end: 6 }]);
		expect(moved.midi_contents).toBe(state.midi_contents);
	});

	it('does not mutate the previous state', () => {
		const before = addClip(addClip(withTracks('t1'), 't1', 'a', [note('n1')]), 't1', 'b');
		const snapshot = structuredClone(before);
		let after = applyEvent(
			before,
			event(ev.MIDI_CONTENT_NOTES_SET, { content_id: 'content-a', notes: [] })
		);
		after = applyEvent(after, event(ev.MIDI_CLIP_REMOVED, { track_id: 't1', clip_id: 'b' }));
		expect(after).not.toBe(before);
		expect(before).toEqual(snapshot);
	});

	describe('linked clips', () => {
		function withRepeat(): ProjectState {
			const state = addClip(withTracks('t1', 't2'), 't1', 'a', [note('n1')]);
			return applyEvent(
				state,
				event(ev.MIDI_CLIP_ADDED, {
					track_id: 't2',
					clip: midiClip('b', 4, 6, 'content-a')
				})
			);
		}

		function unlink(state: ProjectState, trackId: string, clipId: string): ProjectState {
			return applyEvent(
				state,
				event(ev.MIDI_CLIP_UNLINKED, {
					track_id: trackId,
					clip_id: clipId,
					content_id: 'content-copy',
					content: midiContent([note('n1')])
				})
			);
		}

		it('adds a clip on an existing content without storing a second one', () => {
			const state = withRepeat();
			expect(state.tracks[1].midi_clips).toEqual([midiClip('b', 4, 6, 'content-a')]);
			expect(Object.keys(state.midi_contents)).toEqual(['content-a']);
		});

		it('repoints an unlinked clip at its own copy and leaves the others on the original', () => {
			const state = unlink(withRepeat(), 't2', 'b');
			expect(state.tracks[1].midi_clips[0].content_id).toBe('content-copy');
			expect(state.tracks[0].midi_clips[0].content_id).toBe('content-a');
			expect(state.midi_contents['content-copy']).toEqual(midiContent([note('n1')]));
			expect(state.midi_contents['content-a']).toEqual(midiContent([note('n1')]));
		});

		it('edits the copy without touching the original', () => {
			let state = unlink(withRepeat(), 't2', 'b');
			state = applyEvent(
				state,
				event(ev.MIDI_CONTENT_NOTES_SET, { content_id: 'content-copy', notes: [note('n2', 72)] })
			);
			expect(state.midi_contents['content-a'].notes).toEqual([note('n1')]);
		});

		it('prunes the content the clip no longer plays', () => {
			const state = unlink(addClip(withTracks('t1'), 't1', 'a', [note('n1')]), 't1', 'a');
			expect(Object.keys(state.midi_contents)).toEqual(['content-copy']);
		});

		it('ignores an unlink of an unknown clip or track and stores no content', () => {
			const state = withRepeat();
			expect(unlink(state, 't1', 'ghost')).toBe(state);
			expect(unlink(state, 'ghost', 'a')).toBe(state);
		});

		it('does not mutate the previous state', () => {
			const before = withRepeat();
			const snapshot = structuredClone(before);
			const after = unlink(before, 't2', 'b');
			expect(after).not.toBe(before);
			expect(before).toEqual(snapshot);
		});
	});
});

function samplerPad(note: number): SamplerPad {
	return {
		note,
		sample_path: null,
		gain_db: 0,
		pan: 0,
		tune_semitones: 0,
		mode: 'one_shot',
		attack_ms: 0,
		release_ms: 10,
		choke_group: 0,
		missing: false
	};
}

describe('sampler (ADR-028)', () => {
	const kit = { pads: [samplerPad(36), samplerPad(37)] };
	const withTrack = () =>
		replayEvents([event(ev.TRACK_CREATED, { id: 'd', name: 'Drums', instrument: null })]);

	it('takes the kit the engine created when the sampler is chosen', () => {
		const state = applyEvent(
			withTrack(),
			event(ev.TRACK_INSTRUMENT_SET, {
				id: 'd',
				instrument: SAMPLER_INSTRUMENT_ID,
				sampler: kit
			})
		);
		expect(state.tracks[0].instrument).toBe(SAMPLER_INSTRUMENT_ID);
		expect(state.tracks[0].sampler?.pads).toHaveLength(2);
	});

	it('keeps the kit when another instrument is chosen', () => {
		let state = applyEvent(
			withTrack(),
			event(ev.TRACK_INSTRUMENT_SET, { id: 'd', instrument: SAMPLER_INSTRUMENT_ID, sampler: kit })
		);
		state = applyEvent(state, event(ev.TRACK_INSTRUMENT_SET, { id: 'd', instrument: null }));
		expect(state.tracks[0].instrument).toBeNull();
		expect(state.tracks[0].sampler).toEqual(kit);
	});

	it('replaces only the edited pad', () => {
		let state = applyEvent(
			withTrack(),
			event(ev.TRACK_INSTRUMENT_SET, { id: 'd', instrument: SAMPLER_INSTRUMENT_ID, sampler: kit })
		);
		const { missing: _missing, ...settings } = samplerPad(37);
		state = applyEvent(
			state,
			event(ev.SAMPLER_PAD_SET, {
				track_id: 'd',
				pad: 1,
				settings: { ...settings, sample_path: 'gone.wav', gain_db: -6 },
				missing: true
			})
		);
		const pads = state.tracks[0].sampler!.pads;
		expect(pads[0]).toEqual(kit.pads[0]);
		expect(pads[1]).toMatchObject({ sample_path: 'gone.wav', gain_db: -6, missing: true });
	});

	it('reads the kit from a snapshot', () => {
		const state = projectFromSnapshot({
			...emptyProjectState(),
			tracks: [
				{
					id: 'd',
					name: 'Drums',
					volume: 80,
					pan: 0,
					muted: false,
					solo: false,
					clips: [],
					effects: [],
					modulators: [],
					mod_routes: [],
					midi_notes: [],
					midi_clips: [],
					automation_lanes: [],
					instrument: SAMPLER_INSTRUMENT_ID,
					sampler: kit
				}
			]
		} as Project);
		expect(state.tracks[0].sampler).toEqual(kit);
	});
});

describe('arpeggiator (ADR-030)', () => {
	const settings = {
		enabled: true,
		mode: 'up_down' as const,
		rate: 'eighth' as const,
		triplet: true,
		gate_percent: 60,
		octaves: 3
	};
	const withTrack = () =>
		replayEvents([event(ev.TRACK_CREATED, { id: 'a', name: 'Lead', instrument: null })]);

	it('starts without settings', () => {
		expect(withTrack().tracks[0].arpeggiator).toBeNull();
	});

	it('takes the whole settings the engine sends', () => {
		const state = applyEvent(withTrack(), event(ev.ARPEGGIATOR_SET, { track_id: 'a', settings }));
		expect(state.tracks[0].arpeggiator).toEqual(settings);
	});

	it('ignores a track that no longer exists', () => {
		const before = withTrack();
		const state = applyEvent(before, event(ev.ARPEGGIATOR_SET, { track_id: 'gone', settings }));
		expect(state.tracks).toEqual(before.tracks);
	});

	it('reads the settings from a snapshot, and tolerates their absence', () => {
		const track = {
			name: 'Lead',
			volume: 80,
			pan: 0,
			muted: false,
			solo: false,
			clips: [],
			effects: [],
			modulators: [],
			mod_routes: [],
			midi_notes: [],
			midi_clips: [],
			automation_lanes: []
		};
		const state = projectFromSnapshot({
			...emptyProjectState(),
			tracks: [
				{ ...track, id: 'a', arpeggiator: settings },
				{ ...track, id: 'b' }
			]
		});
		expect(state.tracks[0].arpeggiator).toEqual(settings);
		expect(state.tracks[1].arpeggiator).toBeNull();
	});
});
