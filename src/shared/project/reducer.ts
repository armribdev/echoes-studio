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

import { ev, type EngineEvent } from '../protocol/events';
import type { Clip, MidiClip, MidiNote, Project, ProjectModulationRoute, ProjectModulator, Track, TrackEffect } from '../../types/global';

// ── Project state ─────────────────────────────────────────────────────────────

export interface ProjectState {
  name: string;
  bpm: number;
  numerator: number;
  denominator: number;
  tracks: TrackState[];
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
  clips: ClipState[];
  effects: TrackEffect[];
  modulators: ProjectModulator[];
  mod_routes: ProjectModulationRoute[];
  midi_notes: MidiNote[];
  midi_clips: MidiClip[];
  instrument: string | null;
}

export function emptyProjectState(): ProjectState {
  return {
    name: 'Sans titre',
    bpm: 120,
    numerator: 4,
    denominator: 4,
    tracks: [],
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
      const p = event.payload as { id: string; name: string };
      if (state.tracks.some((t) => t.id === p.id)) return state; // idempotent
      return {
        ...state,
        tracks: [...state.tracks, { id: p.id, name: p.name, volume: 80, pan: 0, muted: false, solo: false, instrument: null, clips: [], effects: [], modulators: [], mod_routes: [], midi_notes: [], midi_clips: [] }],
      };
    }

    case ev.TRACK_DELETED: {
      const p = event.payload as { id: string };
      return { ...state, tracks: state.tracks.filter((t) => t.id !== p.id) };
    }

    case ev.TRACK_UPDATED: {
      const p = event.payload as { id: string; name?: string; volume?: number; pan?: number };
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
              },
        ),
      };
    }

    case ev.TRACK_MUTED: {
      const p = event.payload as { id: string; muted: boolean };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.id ? t : { ...t, muted: p.muted },
        ),
      };
    }

    case ev.TRACK_SOLOED: {
      const p = event.payload as { id: string; solo: boolean };
      if (p.solo) {
        return {
          ...state,
          tracks: state.tracks.map((t) => ({ ...t, solo: t.id === p.id })),
        };
      }
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.id ? t : { ...t, solo: false },
        ),
      };
    }

    case ev.TRACK_INSTRUMENT_SET: {
      const p = event.payload as { id: string; instrument: string | null };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.id ? t : { ...t, instrument: p.instrument ?? null },
        ),
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
                  ? t.clips          // idempotent
                  : [...t.clips, clip],
              },
        ),
      };
    }

    case ev.CLIP_MOVED: {
      const p = event.payload as { track_id: string; clip_id: string; start: number; end: number };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.track_id
            ? t
            : {
                ...t,
                clips: t.clips.map((c) =>
                  c.id !== p.clip_id ? c : { ...c, start: p.start, end: p.end },
                ),
              },
        ),
      };
    }

    case ev.CLIP_REMOVED: {
      const p = event.payload as { track_id: string; clip_id: string };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.track_id
            ? t
            : { ...t, clips: t.clips.filter((c) => c.id !== p.clip_id) },
        ),
      };
    }

    case ev.PROJECT_NAME_UPDATED: {
      const p = event.payload as { name: string };
      return { ...state, name: p.name };
    }

    case ev.TEMPO_UPDATED: {
      const p = event.payload as { bpm: number; numerator: number; denominator: number };
      return { ...state, bpm: p.bpm, numerator: p.numerator, denominator: p.denominator };
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
                  : [...t.effects, p.effect],
              },
        ),
      };
    }

    case ev.EFFECT_REMOVED: {
      const p = event.payload as { track_id: string; effect_id: string };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.track_id
            ? t
            : { ...t, effects: t.effects.filter((e) => e.id !== p.effect_id) },
        ),
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
                  e.id !== p.effect_id ? e : { ...e, enabled: p.effect.enabled, params: p.effect.params },
                ),
              },
        ),
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
                  : [...t.modulators, p.modulator],
              },
        ),
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
                mod_routes: t.mod_routes.filter((r) => r.modulator_id !== p.modulator_id),
              },
        ),
      };
    }

    case ev.MODULATOR_UPDATED: {
      const p = event.payload as { track_id: string; modulator_id: string; rate_hz?: number };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.track_id
            ? t
            : {
                ...t,
                modulators: t.modulators.map((m) =>
                  m.id !== p.modulator_id
                    ? m
                    : { ...m, ...(p.rate_hz != null ? { rate_hz: p.rate_hz } : {}) },
                ),
              },
        ),
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
                  : [...t.mod_routes, p.route],
              },
        ),
      };
    }

    case ev.MODULATION_ROUTE_REMOVED: {
      const p = event.payload as { track_id: string; route_id: string };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.track_id
            ? t
            : { ...t, mod_routes: t.mod_routes.filter((r) => r.id !== p.route_id) },
        ),
      };
    }

    case ev.MIDI_NOTES_SET: {
      const p = event.payload as { track_id: string; notes: MidiNote[] };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.track_id ? t : { ...t, midi_notes: p.notes },
        ),
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
                  : [...t.midi_clips, p.clip],
              },
        ),
      };
    }

    case ev.MIDI_CLIP_REMOVED: {
      const p = event.payload as { track_id: string; clip_id: string };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.track_id
            ? t
            : { ...t, midi_clips: t.midi_clips.filter((c) => c.id !== p.clip_id) },
        ),
      };
    }

    case ev.MIDI_CLIP_MOVED: {
      const p = event.payload as { track_id: string; clip_id: string; start: number; end: number };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.track_id
            ? t
            : {
                ...t,
                midi_clips: t.midi_clips.map((c) =>
                  c.id !== p.clip_id ? c : { ...c, start: p.start, end: p.end },
                ),
              },
        ),
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
                  c.id !== p.clip_id ? c : { ...c, notes: p.notes },
                ),
              },
        ),
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
                  r.id !== p.route_id ? r : { ...r, depth: p.depth },
                ),
              },
        ),
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
  base: ProjectState = emptyProjectState(),
): ProjectState {
  return events.reduce(applyEvent, base);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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
    tracks: dto.tracks.map((t) => ({
      id: t.id,
      name: t.name,
      volume: t.volume,
      pan: t.pan ?? 0,
      muted: t.muted ?? false,
      solo: t.solo ?? false,
      clips: t.clips.map((c) => ({ ...c, trackId: t.id })),
      effects: t.effects ?? [],
      modulators: t.modulators ?? [],
      mod_routes: t.mod_routes ?? [],
      midi_notes: t.midi_notes ?? [],
      midi_clips: t.midi_clips ?? [],
      instrument: t.instrument ?? null,
    })),
  };
}
