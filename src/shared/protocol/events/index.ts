/**
 * Engine event types emitted from the Rust gateway to the frontend.
 *
 * Two categories:
 *
 * - **Project events** (persisted) — the source of truth for project structure.
 *   Replaying them deterministically reconstructs any past project state.
 *
 * - **Runtime events** (transient) — transport, meters, latency warnings.
 *   Never persisted; carry no undo/redo semantics.
 *
 * All events arrive via Tauri's event channel named `"engine-event"`.
 */

import type { Clip, Track } from '../../../types/global';

// ── Event type constants ──────────────────────────────────────────────────────

export const ev = {
  // Project events — persisted
  TRACK_CREATED: 'TRACK_CREATED',
  TRACK_DELETED: 'TRACK_DELETED',
  TRACK_UPDATED: 'TRACK_UPDATED',
  CLIP_ADDED: 'CLIP_ADDED',
  CLIP_MOVED: 'CLIP_MOVED',
  CLIP_REMOVED: 'CLIP_REMOVED',
  PROJECT_NAME_UPDATED: 'PROJECT_NAME_UPDATED',
  TEMPO_UPDATED: 'TEMPO_UPDATED',
  PROJECT_RESET: 'PROJECT_RESET',

  // Transport — runtime only
  PLAY_STARTED: 'PLAY_STARTED',
  PLAY_STOPPED: 'PLAY_STOPPED',
  POSITION_UPDATED: 'POSITION_UPDATED',

  // Audio feedback — UI only
  PEAK_METER: 'PEAK_METER',
  CLIP_PLAYING: 'CLIP_PLAYING',
  LATENCY_WARNING: 'LATENCY_WARNING',
} as const;

export type EvType = (typeof ev)[keyof typeof ev];

// ── Base envelope ─────────────────────────────────────────────────────────────

export interface EngineEvent<P = unknown> {
  id: string;
  projectId: string;
  type: EvType | string;
  timestamp: number;
  payload: P;
}

// ── Typed payload shapes ──────────────────────────────────────────────────────

export interface TrackCreatedPayload {
  id: string;
  name: string;
}

export interface TrackDeletedPayload {
  id: string;
}

export interface TrackUpdatedPayload {
  id: string;
  name?: string;
  volume?: number;
}

export interface ClipAddedPayload {
  track_id: string;
  clip: Clip;
}

export interface ClipMovedPayload {
  track_id: string;
  clip_id: string;
  start: number;
  end: number;
}

export interface ClipRemovedPayload {
  track_id: string;
  clip_id: string;
}

export interface ProjectNameUpdatedPayload {
  name: string;
}

export interface TempoUpdatedPayload {
  bpm: number;
  numerator: number;
  denominator: number;
}

export interface PositionUpdatedPayload {
  position: number;
}

export interface PeakMeterPayload {
  trackId: string;
  peak: number;
  rms: number;
}

// ── Type guards ───────────────────────────────────────────────────────────────

export function isEngineEvent(raw: unknown): raw is EngineEvent {
  return (
    typeof raw === 'object' &&
    raw !== null &&
    'type' in raw &&
    'payload' in raw
  );
}
