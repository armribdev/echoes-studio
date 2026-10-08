/**
 * Pure project reducer.
 *
 * Architecture rules:
 * - PURE: same events always produce the same state.
 * - DETERMINISTIC: no randomness, no side effects.
 * - REPLAYABLE: applying events from index 0 reconstructs any past state.
 *
 * The reducer does NOT handle UI state (selection, panels, hover, drag).
 * Those live exclusively in the frontend UI store.
 */

import {
	ev,
	type EngineEvent,
	type SamplerPadSetPayload,
	type TrackCreatedPayload,
	type TrackInstrumentSetPayload
} from '../protocol/events';
import type {
	ArrangementMarker,
	ArrangementRegion,
	AutomationClip,
	AutomationLane,
	Clip,
	MidiClip,
	MidiNote,
	PluginOutputSource,
	Project,
	SamplerKit,
	ProjectModulationRoute,
	ProjectModulator,
	TrackEffect
} from '../../types/global';

const MODULATOR_FIELDS = [
	'rate_hz',
	'waveform',
	'tempo_sync',
	'beat_division',
	'value',
	'attack_ms',
	'decay_ms',
	'sustain_level',
	'release_ms',
	'window_ms',
	'mode'
] as const satisfies readonly (keyof ProjectModulator)[];

// ── Project state ─────────────────────────────────────────────────────────────

export interface ProjectState {
	name: string;
	bpm: number;
	numerator: number;
	denominator: number;
	midi_grid_beats: number;
	key_root: string;
	key_mode: string;
	swing_percent: number;
	midi_ppq: number;
	midi_grid_use_triplets: boolean;
	tracks: TrackState[];
	markers: ArrangementMarker[];
	regions: ArrangementRegion[];
}

/**
 * Extends `Clip` directly — same snake_case field names as the Rust DTO —
 * so `TrackState` is structurally assignable to `Track` with no cast needed.
 */
export interface ClipState extends Clip {
	/** Track this clip belongs to (added by the event store layer). */
	trackId: string;
}

/**
 * TrackState is structurally compatible with Track:
 *   - same `id`, `name`, `volume` fields
 *   - `clips: ClipState[]` is assignable to `clips: Clip[]` because ClipState extends Clip
 */
export interface TrackState {
	id: string;
	name: string;
	volume: number;
	pan: number;
	muted: boolean;
	solo: boolean;
	color?: string | null;
	clips: ClipState[];
	effects: TrackEffect[];
	modulators: ProjectModulator[];
	mod_routes: ProjectModulationRoute[];
	midi_notes: MidiNote[];
	midi_clips: MidiClip[];
	instrument: string | null;
	sampler: SamplerKit | null;
	input_source: PluginOutputSource | null;
	automation_lanes: AutomationLane[];
}

export function emptyProjectState(): ProjectState {
	return {
		name: 'Sans titre',
		bpm: 120,
		numerator: 4,
		denominator: 4,
		midi_grid_beats: 0.25,
		key_root: 'C',
		key_mode: 'major',
		swing_percent: 0,
		midi_ppq: 960,
		midi_grid_use_triplets: false,
		tracks: [],
		markers: [],
		regions: []
	};
}

// ── Reducer ───────────────────────────────────────────────────────────────────

/**
 * Apply a single engine event to a project state, returning the new state.
 *
 * Does NOT mutate `state` — returns a structurally-shared copy.
 */
export function applyEvent(state: ProjectState, event: EngineEvent): ProjectState {
	switch (event.type) {
		case ev.TRACK_CREATED: {
			const p = event.payload as TrackCreatedPayload;
			if (state.tracks.some((t) => t.id === p.id)) return state; // idempotent
			return {
				...state,
				tracks: [
					...state.tracks,
					{
						id: p.id,
						name: p.name,
						volume: 80,
						pan: 0,
						muted: false,
						solo: false,
						color: null,
						instrument: p.instrument ?? null,
						sampler: p.sampler ?? null,
						input_source: null,
						clips: [],
						effects: [],
						modulators: [],
						mod_routes: [],
						midi_notes: [],
						midi_clips: [],
						automation_lanes: []
					}
				]
			};
		}

		case ev.TRACK_DELETED: {
			const p = event.payload as { id: string };
			return { ...state, tracks: state.tracks.filter((t) => t.id !== p.id) };
		}

		case ev.TRACK_UPDATED: {
			const p = event.payload as {
				id: string;
				name?: string;
				volume?: number;
				pan?: number;
				color?: string;
			};
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.id
						? t
						: {
								...t,
								name: p.name ?? t.name,
								volume: p.volume ?? t.volume,
								pan: p.pan ?? t.pan,
								color: p.color ?? t.color
							}
				)
			};
		}

		case ev.TRACK_MUTED: {
			const p = event.payload as { id: string; muted: boolean };
			return {
				...state,
				tracks: state.tracks.map((t) => (t.id !== p.id ? t : { ...t, muted: p.muted }))
			};
		}

		case ev.TRACK_SOLOED: {
			const p = event.payload as { id: string; solo: boolean };
			if (p.solo) {
				return {
					...state,
					tracks: state.tracks.map((t) => ({ ...t, solo: t.id === p.id }))
				};
			}
			return {
				...state,
				tracks: state.tracks.map((t) => (t.id !== p.id ? t : { ...t, solo: false }))
			};
		}

		case ev.TRACK_INSTRUMENT_SET: {
			const p = event.payload as TrackInstrumentSetPayload;
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.id
						? t
						: { ...t, instrument: p.instrument ?? null, sampler: p.sampler ?? t.sampler }
				)
			};
		}

		case ev.SAMPLER_PAD_SET: {
			const p = event.payload as SamplerPadSetPayload;
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id || !t.sampler
						? t
						: {
								...t,
								sampler: {
									pads: t.sampler.pads.map((pad, index) =>
										index !== p.pad ? pad : { ...p.settings, missing: p.missing }
									)
								}
							}
				)
			};
		}

		case ev.TRACK_INPUT_SET: {
			const p = event.payload as { id: string; input_source: PluginOutputSource | null };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.id ? t : { ...t, input_source: p.input_source ?? null }
				)
			};
		}

		case ev.CLIP_ADDED: {
			// payload.clip is a Rust ClipDto (snake_case) — spread directly onto ClipState
			const p = event.payload as { track_id: string; clip: Clip };
			const clip: ClipState = { ...p.clip, trackId: p.track_id };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								clips: t.clips.some((c) => c.id === clip.id)
									? t.clips // idempotent
									: [...t.clips, clip]
							}
				)
			};
		}

		case ev.CLIP_MOVED: {
			const p = event.payload as {
				track_id: string;
				clip_id: string;
				start: number;
				end: number;
				offset?: number;
			};
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								clips: t.clips.map((c) =>
									c.id !== p.clip_id
										? c
										: { ...c, start: p.start, end: p.end, offset: p.offset ?? c.offset }
								)
							}
				)
			};
		}

		case ev.CLIP_MOVED_TO_TRACK: {
			const p = event.payload as { from_track_id: string; to_track_id: string; clip: Clip };
			const clip: ClipState = { ...p.clip, trackId: p.to_track_id };
			return {
				...state,
				tracks: state.tracks.map((t) => {
					if (t.id === p.from_track_id) {
						return { ...t, clips: t.clips.filter((c) => c.id !== clip.id) };
					}
					if (t.id === p.to_track_id) {
						return {
							...t,
							clips: t.clips.some((c) => c.id === clip.id) ? t.clips : [...t.clips, clip]
						};
					}
					return t;
				})
			};
		}

		case ev.CLIP_REMOVED: {
			const p = event.payload as { track_id: string; clip_id: string };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id ? t : { ...t, clips: t.clips.filter((c) => c.id !== p.clip_id) }
				)
			};
		}

		case ev.PROJECT_NAME_UPDATED: {
			const p = event.payload as { name: string };
			return { ...state, name: p.name };
		}

		case ev.TEMPO_UPDATED: {
			const p = event.payload as {
				bpm: number;
				numerator: number;
				denominator: number;
				midi_grid_beats?: number;
				key_root?: string;
				key_mode?: string;
				swing_percent?: number;
				midi_ppq?: number;
				midi_grid_use_triplets?: boolean;
			};
			return {
				...state,
				bpm: p.bpm,
				numerator: p.numerator,
				denominator: p.denominator,
				midi_grid_beats: p.midi_grid_beats ?? state.midi_grid_beats,
				key_root: p.key_root ?? state.key_root,
				key_mode: p.key_mode ?? state.key_mode,
				swing_percent: p.swing_percent ?? state.swing_percent,
				midi_ppq: p.midi_ppq ?? state.midi_ppq,
				midi_grid_use_triplets: p.midi_grid_use_triplets ?? state.midi_grid_use_triplets
			};
		}

		case ev.MARKER_ADDED: {
			const p = event.payload as { marker: ArrangementMarker };
			if (state.markers.some((m) => m.id === p.marker.id)) return state;
			return { ...state, markers: [...state.markers, p.marker] };
		}

		case ev.MARKER_UPDATED: {
			const p = event.payload as { marker: ArrangementMarker };
			return {
				...state,
				markers: state.markers.map((m) => (m.id === p.marker.id ? p.marker : m))
			};
		}

		case ev.MARKER_REMOVED: {
			const p = event.payload as { id: string };
			return { ...state, markers: state.markers.filter((m) => m.id !== p.id) };
		}

		case ev.REGION_ADDED: {
			const p = event.payload as { region: ArrangementRegion };
			if (state.regions.some((r) => r.id === p.region.id)) return state;
			return { ...state, regions: [...state.regions, p.region] };
		}

		case ev.REGION_UPDATED: {
			const p = event.payload as { region: ArrangementRegion };
			return {
				...state,
				regions: state.regions.map((r) => (r.id === p.region.id ? p.region : r))
			};
		}

		case ev.REGION_REMOVED: {
			const p = event.payload as { id: string };
			return { ...state, regions: state.regions.filter((r) => r.id !== p.id) };
		}

		case ev.PROJECT_RESET: {
			// Full snapshot — sent after undo/redo
			return projectFromSnapshot(event.payload as Project);
		}

		case ev.EFFECT_ADDED: {
			const p = event.payload as { track_id: string; effect: TrackEffect };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								effects: t.effects.some((e) => e.id === p.effect.id)
									? t.effects
									: [...t.effects, p.effect]
							}
				)
			};
		}

		case ev.EFFECT_REMOVED: {
			const p = event.payload as { track_id: string; effect_id: string };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id ? t : { ...t, effects: t.effects.filter((e) => e.id !== p.effect_id) }
				)
			};
		}

		case ev.EFFECT_UPDATED: {
			const p = event.payload as { track_id: string; effect_id: string; effect: TrackEffect };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								effects: t.effects.map((e) =>
									e.id !== p.effect_id
										? e
										: { ...e, enabled: p.effect.enabled, params: p.effect.params }
								)
							}
				)
			};
		}

		// ── Modulation ─────────────────────────────────────────────────────────────

		case ev.MODULATOR_ADDED: {
			const p = event.payload as { track_id: string; modulator: ProjectModulator };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								modulators: t.modulators.some((m) => m.id === p.modulator.id)
									? t.modulators
									: [...t.modulators, p.modulator]
							}
				)
			};
		}

		case ev.MODULATOR_REMOVED: {
			const p = event.payload as { track_id: string; modulator_id: string };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								modulators: t.modulators.filter((m) => m.id !== p.modulator_id),
								mod_routes: t.mod_routes.filter((r) => r.modulator_id !== p.modulator_id)
							}
				)
			};
		}

		case ev.MODULATOR_UPDATED: {
			const { track_id, modulator_id, ...fields } = event.payload as {
				track_id: string;
				modulator_id: string;
			} & Partial<Record<(typeof MODULATOR_FIELDS)[number], unknown>>;
			// The engine sends every field, null when unchanged.
			const changes = Object.fromEntries(
				MODULATOR_FIELDS.filter((key) => fields[key] != null).map((key) => [key, fields[key]])
			);
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== track_id
						? t
						: {
								...t,
								modulators: t.modulators.map((m) =>
									m.id !== modulator_id ? m : { ...m, ...changes }
								)
							}
				)
			};
		}

		case ev.MODULATION_ROUTE_ADDED: {
			const p = event.payload as { track_id: string; route: ProjectModulationRoute };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								mod_routes: t.mod_routes.some((r) => r.id === p.route.id)
									? t.mod_routes
									: [...t.mod_routes, p.route]
							}
				)
			};
		}

		case ev.MODULATION_ROUTE_REMOVED: {
			const p = event.payload as { track_id: string; route_id: string };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: { ...t, mod_routes: t.mod_routes.filter((r) => r.id !== p.route_id) }
				)
			};
		}

		case ev.MIDI_NOTES_SET: {
			const p = event.payload as { track_id: string; notes: MidiNote[] };
			return {
				...state,
				tracks: state.tracks.map((t) => (t.id !== p.track_id ? t : { ...t, midi_notes: p.notes }))
			};
		}

		case ev.MIDI_CLIP_ADDED: {
			const p = event.payload as { track_id: string; clip: MidiClip };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								midi_clips: t.midi_clips.some((c) => c.id === p.clip.id)
									? t.midi_clips
									: [...t.midi_clips, p.clip]
							}
				)
			};
		}

		case ev.MIDI_CLIP_REMOVED: {
			const p = event.payload as { track_id: string; clip_id: string };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: { ...t, midi_clips: t.midi_clips.filter((c) => c.id !== p.clip_id) }
				)
			};
		}

		case ev.MIDI_CLIP_MOVED: {
			const p = event.payload as {
				track_id: string;
				clip_id: string;
				start: number;
				end: number;
				offset?: number;
			};
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								midi_clips: t.midi_clips.map((c) =>
									c.id !== p.clip_id
										? c
										: { ...c, start: p.start, end: p.end, offset: p.offset ?? c.offset }
								)
							}
				)
			};
		}

		case ev.MIDI_CLIP_MOVED_TO_TRACK: {
			const p = event.payload as { from_track_id: string; to_track_id: string; clip: MidiClip };
			return {
				...state,
				tracks: state.tracks.map((t) => {
					if (t.id === p.from_track_id) {
						return { ...t, midi_clips: t.midi_clips.filter((c) => c.id !== p.clip.id) };
					}
					if (t.id === p.to_track_id) {
						return {
							...t,
							midi_clips: t.midi_clips.some((c) => c.id === p.clip.id)
								? t.midi_clips
								: [...t.midi_clips, p.clip]
						};
					}
					return t;
				})
			};
		}

		case ev.MIDI_CLIP_NOTES_SET: {
			const p = event.payload as { track_id: string; clip_id: string; notes: MidiNote[] };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								midi_clips: t.midi_clips.map((c) =>
									c.id !== p.clip_id ? c : { ...c, notes: p.notes }
								)
							}
				)
			};
		}

		case ev.MODULATION_DEPTH_CHANGED: {
			const p = event.payload as { track_id: string; route_id: string; depth: number };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								mod_routes: t.mod_routes.map((r) =>
									r.id !== p.route_id ? r : { ...r, depth: p.depth }
								)
							}
				)
			};
		}

		case ev.AUTOMATION_LANE_ADDED: {
			const p = event.payload as { track_id: string; lane: AutomationLane };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id || t.automation_lanes.some((l) => l.id === p.lane.id)
						? t
						: { ...t, automation_lanes: [...t.automation_lanes, p.lane] }
				)
			};
		}

		case ev.AUTOMATION_LANE_REMOVED: {
			const p = event.payload as { track_id: string; lane_id: string };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: { ...t, automation_lanes: t.automation_lanes.filter((l) => l.id !== p.lane_id) }
				)
			};
		}

		case ev.AUTOMATION_CLIP_UPDATED: {
			const p = event.payload as { track_id: string; lane_id: string; clip: AutomationClip };
			return {
				...state,
				tracks: state.tracks.map((t) =>
					t.id !== p.track_id
						? t
						: {
								...t,
								automation_lanes: t.automation_lanes.map((l) =>
									l.id !== p.lane_id ? l : { ...l, clips: upsertById(l.clips, p.clip) }
								)
							}
				)
			};
		}

		default:
			return state; // forward-compat: unknown events ignored
	}
}

/**
 * Replay a sequence of events from an optional base state.
 */
export function replayEvents(
	events: EngineEvent[],
	base: ProjectState = emptyProjectState()
): ProjectState {
	return events.reduce(applyEvent, base);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function upsertById<T extends { id: string }>(items: T[], item: T): T[] {
	return items.some((i) => i.id === item.id)
		? items.map((i) => (i.id === item.id ? item : i))
		: [...items, item];
}

/**
 * Build a ProjectState from a raw backend DTO
 * (`get_project` / `get_snapshot` / `project-loaded`).
 */
export function projectFromSnapshot(dto: Project): ProjectState {
	return {
		name: dto.name,
		bpm: dto.bpm,
		numerator: dto.numerator,
		denominator: dto.denominator,
		midi_grid_beats: dto.midi_grid_beats ?? 0.25,
		key_root: dto.key_root ?? 'C',
		key_mode: dto.key_mode ?? 'major',
		swing_percent: dto.swing_percent ?? 0,
		midi_ppq: dto.midi_ppq ?? 960,
		midi_grid_use_triplets: dto.midi_grid_use_triplets ?? false,
		tracks: dto.tracks.map((t) => ({
			id: t.id,
			name: t.name,
			volume: t.volume,
			pan: t.pan ?? 0,
			muted: t.muted ?? false,
			solo: t.solo ?? false,
			color: t.color ?? null,
			clips: t.clips.map((c) => ({ ...c, trackId: t.id })),
			effects: t.effects ?? [],
			modulators: t.modulators ?? [],
			mod_routes: t.mod_routes ?? [],
			midi_notes: t.midi_notes ?? [],
			midi_clips: t.midi_clips ?? [],
			instrument: t.instrument ?? null,
			sampler: t.sampler ?? null,
			input_source: t.input_source ?? null,
			automation_lanes: t.automation_lanes ?? []
		})),
		markers: dto.markers ?? [],
		regions: dto.regions ?? []
	};
}
