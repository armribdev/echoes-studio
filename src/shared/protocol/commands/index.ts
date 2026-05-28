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

// ── Command type constants ────────────────────────────────────────────────────

export const cmd = {
  // Transport (runtime only — not persisted in event store)
  PLAY: 'PLAY',
  STOP: 'STOP',
  PAUSE: 'PAUSE',
  SEEK: 'SEEK',

  // Project structure (event-sourced)
  CREATE_TRACK: 'CREATE_TRACK',
  DELETE_TRACK: 'DELETE_TRACK',
  UPDATE_TRACK: 'UPDATE_TRACK',
  MUTE_TRACK: 'MUTE_TRACK',
  SOLO_TRACK: 'SOLO_TRACK',
  ADD_CLIP: 'ADD_CLIP',
  MOVE_CLIP: 'MOVE_CLIP',
  REMOVE_CLIP: 'REMOVE_CLIP',
  UPDATE_PROJECT_NAME: 'UPDATE_PROJECT_NAME',
  UPDATE_TEMPO: 'UPDATE_TEMPO',

  // Effects
  ADD_EFFECT: 'ADD_EFFECT',
  REMOVE_EFFECT: 'REMOVE_EFFECT',
  UPDATE_EFFECT: 'UPDATE_EFFECT',

  // Audio graph (structural routing — future)
  ADD_NODE: 'ADD_NODE',
  REMOVE_NODE: 'REMOVE_NODE',
  CONNECT_NODES: 'CONNECT_NODES',

  // Modulation
  ADD_MODULATOR: 'ADD_MODULATOR',
  REMOVE_MODULATOR: 'REMOVE_MODULATOR',
  UPDATE_MODULATOR: 'UPDATE_MODULATOR',
  ADD_MODULATION_ROUTE: 'ADD_MODULATION_ROUTE',
  REMOVE_MODULATION_ROUTE: 'REMOVE_MODULATION_ROUTE',
  SET_MODULATION_DEPTH: 'SET_MODULATION_DEPTH',

  // MIDI
  SET_MIDI_NOTES: 'SET_MIDI_NOTES',

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

export interface AddClipPayload {
  track_id: string;
  file_path: string;
  start: number;
  end: number;
}

export interface MoveClipPayload {
  track_id: string;
  clip_id: string;
  start: number;
  end: number;
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
}

export interface SeekPayload {
  position: number;
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
}

// ── Modulation payloads ───────────────────────────────────────────────────────

export interface AddModulatorPayload {
  track_id: string;
  kind: 'lfo';
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
}

export interface AddModulationRoutePayload {
  track_id: string;
  modulator_id: string;
  target_kind: 'track_gain' | 'effect_param';
  effect_id?: string;
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

export interface SetMidiNotesPayload {
  track_id: string;
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
