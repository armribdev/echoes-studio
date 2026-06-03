/**
 * Svelte-compatible engine store.
 *
 * Wraps the {@link ProjectEventStore} singleton in a Svelte `readable` so
 * components can subscribe with `$engineProject` syntax.
 *
 * UI-only state (selection, panels, drag) is intentionally NOT here — it
 * lives in the existing stores under `src/stores/`.
 *
 * Architecture rule: this store is DERIVED from events. Components must
 * never directly mutate it. All mutations go through the command dispatcher.
 */

import { readable, derived, type Readable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { projectEventStore } from '../../shared/project/eventStore';
import { ev, type EngineEvent } from '../../shared/protocol/events';
import {
  emptyProjectState,
  projectFromSnapshot,
  type ProjectState,
  type TrackState,
} from '../../shared/project/reducer';
import type { Project } from '../../types/global';

// ── Engine project state (derived from events) ────────────────────────────────

/**
 * Svelte readable store that reflects the current engine-side project state.
 *
 * Initialised from `get_project` / `get_snapshot` and then kept in sync by
 * the event store's subscription.
 */
export const engineProject: Readable<ProjectState> = readable(
  emptyProjectState(),
  (set) => {
    // Subscribe to event store
    const unsub = projectEventStore.subscribe(set);

    // Hydrate from backend on first mount
    invoke<Project>('get_project')
      .then((dto) => {
        projectEventStore.seedFromSnapshot(dto);
      })
      .catch(console.error);

    // Start receiving engine events
    projectEventStore.connect().catch(console.error);

    return () => {
      unsub();
      projectEventStore.disconnect();
    };
  },
);

// ── Runtime event store (transport / meters) ──────────────────────────────────

/**
 * Raw stream of runtime engine events (PLAY_STARTED, POSITION_UPDATED, etc.).
 * Components that need real-time feedback (transport bar, level meters) should
 * subscribe here instead of polling.
 *
 * These events are NOT persisted and have no undo/redo semantics.
 */
export const engineRuntimeEvents: Readable<EngineEvent | null> = readable<EngineEvent | null>(
  null,
  (set) => {
    let unlisten: UnlistenFn | null = null;

    listen<EngineEvent>('engine-event', ({ payload }) => {
      // Forward only runtime events
      if (
        payload.type === ev.PLAY_STARTED ||
        payload.type === ev.PLAY_STOPPED ||
        payload.type === ev.POSITION_UPDATED ||
        payload.type === ev.PEAK_METER ||
        payload.type === ev.CLIP_PLAYING ||
        payload.type === ev.LATENCY_WARNING
      ) {
        set(payload);
      }
    })
      .then((fn) => (unlisten = fn))
      .catch(console.error);

    return () => {
      unlisten?.();
    };
  },
);

// ── Convenience derived stores ─────────────────────────────────────────────────

/** All track states from the engine project. */
export const engineTracks: Readable<TrackState[]> = derived(
  engineProject,
  ($p) => $p.tracks,
);

/** Project tempo derived from engine state. */
export const engineTempo: Readable<{ bpm: number; numerator: number; denominator: number }> =
  derived(engineProject, ($p) => ({
    bpm: $p.bpm,
    numerator: $p.numerator,
    denominator: $p.denominator,
  }));

export const engineMidiGridBeats: Readable<number> = derived(
  engineProject,
  ($p) => $p.midi_grid_beats ?? 0.25,
);

/** Project name derived from engine state. */
export const engineProjectName: Readable<string> = derived(
  engineProject,
  ($p) => $p.name,
);
