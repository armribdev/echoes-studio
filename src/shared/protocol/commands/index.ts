/**
 * Typed command envelope and payload shapes.
 *
 * Every user action that mutates project state is expressed as a
 * {@link CommandEnvelope}. The gateway dispatches on `type` and
 * deserialises the payload for that command branch.
 *
 * Usage:
 *   import { cmd, makeCommand } from '$shared/protocol/commands';
 *   const command = makeCommand(cmd.CREATE_TRACK, projectId, { name: 'Lead' });
 *   await dispatch(command);
 */

import type { AutomationClip, InterpolationMode, PluginOutputSource } from '../../../types/global';

// ── Command type constants ────────────────────────────────────────────────────

export const cmd = {
  // Transport (runtime only — not persisted in event store)
  PLAY: 'PLAY',
  STOP: 'STOP',
  PAUSE: 'PAUSE',
  SEEK: 'SEEK',
  SET_LOOP: 'SET_LOOP',

  // Project structure (event-sourced)
  CREATE_TRACK: 'CREATE_TRACK',
  DELETE_TRACK: 'DELETE_TRACK',
  UPDATE_TRACK: 'UPDATE_TRACK',
  MUTE_TRACK: 'MUTE_TRACK',
  SOLO_TRACK: 'SOLO_TRACK',
  SET_TRACK_INSTRUMENT: 'SET_TRACK_INSTRUMENT',
  SET_TRACK_INPUT: 'SET_TRACK_INPUT',
  ADD_CLIP: 'ADD_CLIP',
  MOVE_CLIP: 'MOVE_CLIP',
  REMOVE_CLIP: 'REMOVE_CLIP',
  UPDATE_PROJECT_NAME: 'UPDATE_PROJECT_NAME',
  UPDATE_TEMPO: 'UPDATE_TEMPO',

  // Effects
  ADD_EFFECT: 'ADD_EFFECT',
  REMOVE_EFFECT: 'REMOVE_EFFECT',
  UPDATE_EFFECT: 'UPDATE_EFFECT',

  // Modulation
  ADD_MODULATOR: 'ADD_MODULATOR',
  REMOVE_MODULATOR: 'REMOVE_MODULATOR',
  UPDATE_MODULATOR: 'UPDATE_MODULATOR',
  ADD_MODULATION_ROUTE: 'ADD_MODULATION_ROUTE',
  REMOVE_MODULATION_ROUTE: 'REMOVE_MODULATION_ROUTE',
  SET_MODULATION_DEPTH: 'SET_MODULATION_DEPTH',

  // Automation
  ADD_AUTOMATION_LANE: 'ADD_AUTOMATION_LANE',
  REMOVE_AUTOMATION_LANE: 'REMOVE_AUTOMATION_LANE',
  UPDATE_AUTOMATION_CLIP: 'UPDATE_AUTOMATION_CLIP',

  // MIDI
  SET_MIDI_NOTES: 'SET_MIDI_NOTES',
  ADD_MIDI_CLIP: 'ADD_MIDI_CLIP',
  REMOVE_MIDI_CLIP: 'REMOVE_MIDI_CLIP',
  MOVE_MIDI_CLIP: 'MOVE_MIDI_CLIP',
  SET_CLIP_MIDI_NOTES: 'SET_CLIP_MIDI_NOTES',

  // Batch
  BATCH: 'BATCH',
} as const;

export type CmdType = (typeof cmd)[keyof typeof cmd];

// ── Envelope ──────────────────────────────────────────────────────────────────

/** The outer wrapper for every command sent to the engine gateway. */
export interface CommandEnvelope<P = unknown> {
  /** Caller-generated ID for request-response correlation. */
  id: string;
  /** ID of the project this command targets. */
  projectId: string;
  /** Discriminant — see `cmd.*` constants. */
  type: CmdType | string;
  /** Unix timestamp in milliseconds. */
  timestamp: number;
  /** Command-specific payload. */
  payload: P;
}

/** Groups multiple commands into one IPC round-trip (atomic apply). */
export interface BatchCommand {
  commands: CommandEnvelope[];
}

// ── Typed payload shapes ──────────────────────────────────────────────────────

export interface CreateTrackPayload {
  name: string;
  instrument?: string | null;
}

export interface DeleteTrackPayload {
  track_id: string;
}

export interface UpdateTrackPayload {
  track_id: string;
  name?: string;
  volume?: number;
  pan?: number;
}

export interface MuteTrackPayload {
  track_id: string;
  muted: boolean;
}

export interface SoloTrackPayload {
  track_id: string;
  solo: boolean;
}

export interface SetTrackInstrumentPayload {
  track_id: string;
  instrument: string | null;
}

export interface SetTrackInputPayload {
  track_id: string;
  /** Aux output of a multi-out plugin fed into the track; null for none. */
  input_source: PluginOutputSource | null;
}

export interface AddClipPayload {
  track_id: string;
  file_path: string;
  start: number;
  end: number;
  /** Source-file trim in seconds. Omit to start at the beginning of the file. */
  offset?: number;
}

export interface MoveClipPayload {
  track_id: string;
  clip_id: string;
  start: number;
  end: number;
  /** Source-file trim offset in seconds. Omit on a plain move (offset preserved). */
  offset?: number;
  /** Destination track ID. Omit or same as track_id for a same-track move. */
  target_track_id?: string;
}

export interface RemoveClipPayload {
  track_id: string;
  clip_id: string;
}

export interface UpdateProjectNamePayload {
  name: string;
}

export interface UpdateTempoPayload {
  bpm: number;
  numerator: number;
  denominator: number;
  midi_grid_beats: number;
  key_root: string;
  key_mode: string;
  swing_percent: number;
  midi_ppq: number;
  midi_grid_use_triplets: boolean;
}

export interface SeekPayload {
  position: number;
}

export interface SetLoopPayload {
  enabled: boolean;
  /** Loop start, in seconds. */
  start: number;
  /** Loop end, in seconds. */
  end: number;
}

export interface AddEffectPayload {
  track_id: string;
  kind: string;
}

export interface RemoveEffectPayload {
  track_id: string;
  effect_id: string;
}

export interface UpdateEffectPayload {
  track_id: string;
  effect_id: string;
  enabled?: boolean;
  params?: Record<string, unknown>;
  /** CLAP plugins: run the plugin in its own process (sandbox). */
  sandboxed?: boolean;
}

// ── Modulation payloads ───────────────────────────────────────────────────────

export interface AddModulatorPayload {
  track_id: string;
  kind: 'lfo' | 'envelope' | 'audio_follower' | 'macro';
  rate_hz?: number;
}

export interface RemoveModulatorPayload {
  track_id: string;
  modulator_id: string;
}

export interface UpdateModulatorPayload {
  track_id: string;
  modulator_id: string;
  rate_hz?: number;
  waveform?: 'sine' | 'triangle' | 'saw' | 'square';
  tempo_sync?: boolean;
  beat_division?: number;
  value?: number;
  attack_ms?: number;
  decay_ms?: number;
  sustain_level?: number;
  release_ms?: number;
  window_ms?: number;
  mode?: 'rms' | 'peak';
  /** `null` taps the track post-FX output. */
  source_effect_id?: string | null;
}

export interface AddModulationRoutePayload {
  track_id: string;
  modulator_id: string;
  target_kind: 'track_gain' | 'effect_param' | 'plugin_param';
  /** Effect id, or `instrument:{track_id}` for the track's plugin instrument. */
  effect_id?: string;
  /** Effect param name, or the plugin native_id as text for `plugin_param`. */
  param: string;
  depth: number;
}

export interface RemoveModulationRoutePayload {
  track_id: string;
  route_id: string;
}

export interface SetModulationDepthPayload {
  track_id: string;
  route_id: string;
  depth: number;
}

export interface AddAutomationLanePayload {
  track_id: string;
  target_kind: 'track_gain' | 'effect_param' | 'plugin_param';
  /** Effect id, or `instrument:{track_id}` for the track's plugin instrument. */
  effect_id?: string;
  /** Effect param name, or the plugin native_id as text for `plugin_param`. */
  param: string;
  interpolation?: InterpolationMode;
}

export interface RemoveAutomationLanePayload {
  track_id: string;
  lane_id: string;
}

/** Replaces the clip with the same id in the lane, or adds it. */
export interface UpdateAutomationClipPayload {
  track_id: string;
  lane_id: string;
  clip: AutomationClip;
}

export interface SetMidiNotesPayload {
  track_id: string;
  notes: { id: string; pitch: number; start: number; duration: number; velocity: number }[];
}

export interface AddMidiClipPayload {
  track_id: string;
  start: number;
  end: number;
  /** Left-trim offset in seconds. Omit for an untrimmed clip. */
  offset?: number;
  /** Notes seeded with the clip (paste). Omit for an empty clip. */
  notes?: { id: string; pitch: number; start: number; duration: number; velocity: number }[];
}

export interface RemoveMidiClipPayload {
  track_id: string;
  clip_id: string;
}

export interface MoveMidiClipPayload {
  track_id: string;
  clip_id: string;
  start: number;
  end: number;
  /** Left-trim offset in seconds. Omit on a plain move (offset preserved). */
  offset?: number;
  /** Destination track ID. Omit or same as track_id for a same-track move. */
  target_track_id?: string;
}

export interface SetClipMidiNotesPayload {
  track_id: string;
  clip_id: string;
  notes: { id: string; pitch: number; start: number; duration: number; velocity: number }[];
}

// ── Factory ───────────────────────────────────────────────────────────────────

let _seq = 0;

/**
 * Create a well-formed {@link CommandEnvelope}.
 *
 * @param type    Command discriminant (use `cmd.*` constants).
 * @param projectId  Active project ID.
 * @param payload Command-specific data.
 */
export function makeCommand<P>(
  type: CmdType | string,
  projectId: string,
  payload: P,
): CommandEnvelope<P> {
  return {
    id: `cmd-${Date.now()}-${++_seq}`,
    projectId,
    type,
    timestamp: Date.now(),
    payload,
  };
}

/** The response returned by the gateway for each command. */
export interface CommandResult {
  success: boolean;
  commandId: string;
  events: unknown[];
  error?: string;
}
