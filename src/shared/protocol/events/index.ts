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
 * - **Mixer events** (`TRACK_MUTED`, `TRACK_SOLOED`) — update live mix state
 *   and are saved with the project file, but are not appended to the event
 *   store and therefore have no undo/redo step.
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
  TRACK_MUTED: 'TRACK_MUTED',
  TRACK_SOLOED: 'TRACK_SOLOED',
  TRACK_INSTRUMENT_SET: 'TRACK_INSTRUMENT_SET',
  CLIP_ADDED: 'CLIP_ADDED',
  CLIP_MOVED: 'CLIP_MOVED',
  CLIP_REMOVED: 'CLIP_REMOVED',
  PROJECT_NAME_UPDATED: 'PROJECT_NAME_UPDATED',
  TEMPO_UPDATED: 'TEMPO_UPDATED',
  PROJECT_RESET: 'PROJECT_RESET',
  EFFECT_ADDED: 'EFFECT_ADDED',
  EFFECT_REMOVED: 'EFFECT_REMOVED',
  EFFECT_UPDATED: 'EFFECT_UPDATED',

  // Modulation — persisted
  MODULATOR_ADDED: 'MODULATOR_ADDED',
  MODULATOR_REMOVED: 'MODULATOR_REMOVED',
  MODULATOR_UPDATED: 'MODULATOR_UPDATED',
  MODULATION_ROUTE_ADDED: 'MODULATION_ROUTE_ADDED',
  MODULATION_ROUTE_REMOVED: 'MODULATION_ROUTE_REMOVED',
  MODULATION_DEPTH_CHANGED: 'MODULATION_DEPTH_CHANGED',

  // MIDI — persisted
  MIDI_NOTES_SET: 'MIDI_NOTES_SET',
  MIDI_CLIP_ADDED: 'MIDI_CLIP_ADDED',
  MIDI_CLIP_REMOVED: 'MIDI_CLIP_REMOVED',
  MIDI_CLIP_MOVED: 'MIDI_CLIP_MOVED',
  MIDI_CLIP_NOTES_SET: 'MIDI_CLIP_NOTES_SET',

  // Transport — runtime only
  PLAY_STARTED: 'PLAY_STARTED',
  PLAY_STOPPED: 'PLAY_STOPPED',
  POSITION_UPDATED: 'POSITION_UPDATED',
  LEVELS_UPDATED: 'LEVELS_UPDATED',

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
  pan?: number;
}

export interface TrackMutedPayload {
  id: string;
  muted: boolean;
}

export interface TrackSoloedPayload {
  id: string;
  solo: boolean;
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
  midi_grid_beats: number;
  key_root?: string;
  key_mode?: string;
  swing_percent?: number;
  midi_ppq?: number;
  midi_grid_use_triplets?: boolean;
}

/** Per-track peak levels: track_id → [peakL, peakR] in linear amplitude 0–1. */
export type TrackLevelsMap = Record<string, [number, number]>;

export interface PositionUpdatedPayload {
  position: number;
  tracks?: TrackLevelsMap;
  /** Cumulative underrun count for the current audio stream session. */
  xruns?: number;
}

export interface LevelsUpdatedPayload {
  output: number[];
  input: number[];
  tracks?: TrackLevelsMap;
  /** Cumulative underrun count for the current audio stream session. */
  xruns?: number;
}

export interface PeakMeterPayload {
  trackId: string;
  peak: number;
  rms: number;
}

// ── Modulation payload shapes ─────────────────────────────────────────────────

export interface LfoParams {
  rate_hz: number;
  waveform: 'sine' | 'triangle' | 'saw' | 'square';
}

export type ModulatorParams =
  | { kind: 'lfo' } & LfoParams;

export interface ProjectModulator {
  id: string;
  kind: string;
  rate_hz?: number;
  waveform?: string;
}

export type ModulationTargetRef =
  | { kind: 'track_gain'; track_id: string }
  | { kind: 'effect_param'; track_id: string; effect_id: string; param: string };

export interface ProjectModulationRoute {
  id: string;
  modulator_id: string;
  target: ModulationTargetRef;
  depth: number;
}

export interface ModulatorAddedPayload {
  track_id: string;
  modulator: ProjectModulator;
}

export interface ModulatorRemovedPayload {
  track_id: string;
  modulator_id: string;
}

export interface ModulatorUpdatedPayload {
  track_id: string;
  modulator_id: string;
  rate_hz?: number;
}

export interface ModulationRouteAddedPayload {
  track_id: string;
  route: ProjectModulationRoute;
}

export interface ModulationRouteRemovedPayload {
  track_id: string;
  route_id: string;
}

export interface ModulationDepthChangedPayload {
  track_id: string;
  route_id: string;
  depth: number;
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
