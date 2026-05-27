/**
 * Frontend event store — mirrors the Rust-side event log.
 *
 * Responsibilities:
 * - Receives engine events from the Tauri `"engine-event"` channel.
 * - Listens to `"project-loaded"` (native menu new/open) and re-seeds on change.
 * - Maintains an ordered log of project events for future replay / debug.
 * - Applies each event to `ProjectState` via the pure reducer.
 * - Notifies all subscribers when state changes.
 *
 * Note: the frontend store is a DERIVED CACHE — the Rust backend is the
 * canonical source of truth. This store must never be mutated directly.
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import {
  applyEvent,
  emptyProjectState,
  projectFromSnapshot,
  type ProjectState,
} from './reducer';
import { ev, isEngineEvent, type EngineEvent } from '../protocol/events';
import type { Project } from '../../types/global';

// ── Store ─────────────────────────────────────────────────────────────────────

type Subscriber = (state: ProjectState) => void;

export class ProjectEventStore {
  /** Ordered log of all project events received this session. */
  private _events: EngineEvent[] = [];
  /** Current derived project state. */
  private _state: ProjectState = emptyProjectState();
  /** Registered subscribers. */
  private _subscribers = new Set<Subscriber>();

  private _unlistenEngine: UnlistenFn | null = null;
  private _unlistenProjectLoaded: UnlistenFn | null = null;

  // ── Lifecycle ──────────────────────────────────────────────────────────────

  /**
   * Seed the store from a full backend snapshot.
   * Called on initial hydration and whenever the active project changes
   * (new project, open file, undo/redo that replaces the full state).
   */
  seedFromSnapshot(dto: Project): void {
    this._state = projectFromSnapshot(dto);
    this._events = []; // snapshot is the new baseline; clear delta log
    this._notify();
  }

  /**
   * Start listening to Tauri event channels.
   * Safe to call multiple times (idempotent).
   */
  async connect(): Promise<void> {
    if (this._unlistenEngine) return; // already connected

    // Engine-event channel — receives all project + runtime events from Rust
    this._unlistenEngine = await listen<EngineEvent>('engine-event', ({ payload }) => {
      this._receive(payload);
    });

    // project-loaded — fired when the user opens a file or creates a new project
    // via the native menu. We re-seed the store so it stays in sync.
    this._unlistenProjectLoaded = await listen<Project>('project-loaded', ({ payload }) => {
      this.seedFromSnapshot(payload);
    });
  }

  /**
   * Stop listening and release resources.
   */
  disconnect(): void {
    this._unlistenEngine?.();
    this._unlistenEngine = null;
    this._unlistenProjectLoaded?.();
    this._unlistenProjectLoaded = null;
  }

  // ── Read ───────────────────────────────────────────────────────────────────

  get state(): ProjectState {
    return this._state;
  }

  get events(): readonly EngineEvent[] {
    return this._events;
  }

  // ── Subscribe (Svelte store contract) ─────────────────────────────────────

  /**
   * Register a subscriber. Returns an unsubscribe function.
   * Immediately calls `fn` with the current state.
   */
  subscribe(fn: Subscriber): () => void {
    this._subscribers.add(fn);
    fn(this._state);
    return () => this._subscribers.delete(fn);
  }

  // ── Internal ───────────────────────────────────────────────────────────────

  private _receive(event: unknown): void {
    if (!isEngineEvent(event)) return;

    // Runtime-only events (transport, meters) don't mutate project state
    if (
      event.type === ev.PLAY_STARTED ||
      event.type === ev.PLAY_STOPPED ||
      event.type === ev.POSITION_UPDATED ||
      event.type === ev.PEAK_METER ||
      event.type === ev.CLIP_PLAYING ||
      event.type === ev.LATENCY_WARNING
    ) {
      return; // handled by engineRuntimeEvents store
    }

    // Mixer events — mutate state but are not undoable (not logged)
    if (event.type === ev.TRACK_MUTED || event.type === ev.TRACK_SOLOED) {
      this._state = applyEvent(this._state, event);
      this._notify();
      return;
    }

    // Project events — append to log and apply to state
    this._events.push(event);
    this._state = applyEvent(this._state, event);
    this._notify();
  }

  private _notify(): void {
    for (const fn of this._subscribers) {
      fn(this._state);
    }
  }
}

/** Application-wide singleton. */
export const projectEventStore = new ProjectEventStore();
