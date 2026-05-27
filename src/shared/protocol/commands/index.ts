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

  // Audio graph (structural routing — future)
  ADD_NODE: 'ADD_NODE',
  REMOVE_NODE: 'REMOVE_NODE',
  CONNECT_NODES: 'CONNECT_NODES',

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
