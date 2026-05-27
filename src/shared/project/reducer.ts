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
import type { Clip, Project, Track } from '../../types/global';

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
  clips: ClipState[];
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
        tracks: [...state.tracks, { id: p.id, name: p.name, volume: 80, clips: [] }],
      };
    }

    case ev.TRACK_DELETED: {
      const p = event.payload as { id: string };
      return { ...state, tracks: state.tracks.filter((t) => t.id !== p.id) };
    }

    case ev.TRACK_UPDATED: {
      const p = event.payload as { id: string; name?: string; volume?: number };
      return {
        ...state,
        tracks: state.tracks.map((t) =>
          t.id !== p.id
            ? t
            : { ...t, name: p.name ?? t.name, volume: p.volume ?? t.volume },
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
      clips: t.clips.map((c) => ({ ...c, trackId: t.id })),
    })),
  };
}
