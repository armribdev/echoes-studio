/**
 * Command dispatcher — the ONLY path for UI-initiated project mutations.
 *
 * Architecture rules:
 * - Components MUST NOT call `invoke()` directly for project mutations.
 * - All mutations go through `dispatch()` or `dispatchBatch()`.
 * - The dispatcher wraps each mutation in a `CommandEnvelope` and sends it
 *   to the Rust engine gateway via `send_command` / `send_batch`.
 * - Transport commands (play/stop/seek) go through `dispatchTransport()`,
 *   which updates the local transport store optimistically at 60fps while
 *   the command propagates to the engine.
 *
 * Latency strategy:
 *   - Transport:     immediate local update + engine notification.
 *   - Clip movement: optimistic local update during drag, deferred commit.
 *   - Structural:    immediate engine round-trip (no local optimistic state).
 */

import { invoke } from '@tauri-apps/api/core';
import {
  cmd,
  makeCommand,
  type AddClipPayload,
  type AddEffectPayload,
  type BatchCommand,
  type CommandEnvelope,
  type CommandResult,
  type CreateTrackPayload,
  type DeleteTrackPayload,
  type MoveClipPayload,
  type MuteTrackPayload,
  type RemoveClipPayload,
  type RemoveEffectPayload,
  type RemoveModulationRoutePayload,
  type RemoveModulatorPayload,
  type SeekPayload,
  type SetModulationDepthPayload,
  type AddModulationRoutePayload,
  type AddModulatorPayload,
  type SoloTrackPayload,
  type UpdateEffectPayload,
  type UpdateModulatorPayload,
  type UpdateProjectNamePayload,
  type UpdateTempoPayload,
  type UpdateTrackPayload,
  type SetMidiNotesPayload,
  type AddMidiClipPayload,
  type RemoveMidiClipPayload,
  type MoveMidiClipPayload,
  type SetClipMidiNotesPayload,
} from '../../shared/protocol/commands';

// ── Active project ID ─────────────────────────────────────────────────────────
//
// The project ID is set once during session initialisation.
// Future: support multiple open projects.

let _projectId = 'default';

export function setActiveProjectId(id: string): void {
  _projectId = id;
}

export function getActiveProjectId(): string {
  return _projectId;
}

// ── Core dispatch ─────────────────────────────────────────────────────────────

/**
 * Send a single command to the engine gateway.
 *
 * @throws if the gateway returns `success: false`.
 */
export async function dispatch<P>(
  type: string,
  payload: P,
  projectId = _projectId,
): Promise<CommandResult> {
  const envelope: CommandEnvelope<P> = makeCommand(type, projectId, payload);
  const result = await invoke<CommandResult>('send_command', { command: envelope });
  if (!result.success) {
    throw new Error(result.error ?? `Command ${type} failed`);
  }
  return result;
}

/**
 * Send multiple commands as a single atomic batch.
 * Useful for grouped edits that should be undone together.
 */
export async function dispatchBatch(
  commands: Array<{ type: string; payload: unknown }>,
  projectId = _projectId,
): Promise<CommandResult[]> {
  const batch: BatchCommand = {
    commands: commands.map(({ type, payload }) =>
      makeCommand(type, projectId, payload),
    ),
  };
  const results = await invoke<CommandResult[]>('send_batch', { batch });
  return results;
}

// ── Typed command helpers ─────────────────────────────────────────────────────
//
// These wrap `dispatch()` with the correct payload type for each command,
// so call sites are type-safe and self-documenting.

// ── Track commands ────────────────────────────────────────────────────────────

export function createTrack(name: string): Promise<CommandResult> {
  return dispatch<CreateTrackPayload>(cmd.CREATE_TRACK, { name });
}

export function deleteTrack(trackId: string): Promise<CommandResult> {
  return dispatch<DeleteTrackPayload>(cmd.DELETE_TRACK, { track_id: trackId });
}

export function updateTrack(
  trackId: string,
  changes: { name?: string; volume?: number; pan?: number },
): Promise<CommandResult> {
  return dispatch<UpdateTrackPayload>(cmd.UPDATE_TRACK, {
    track_id: trackId,
    ...changes,
  });
}

export function muteTrack(trackId: string, muted: boolean): Promise<CommandResult> {
  return dispatch<MuteTrackPayload>(cmd.MUTE_TRACK, { track_id: trackId, muted });
}

export function soloTrack(trackId: string, solo: boolean): Promise<CommandResult> {
  return dispatch<SoloTrackPayload>(cmd.SOLO_TRACK, { track_id: trackId, solo });
}

// ── Clip commands ─────────────────────────────────────────────────────────────

export function addClip(
  trackId: string,
  filePath: string,
  start: number,
  end: number,
): Promise<CommandResult> {
  return dispatch<AddClipPayload>(cmd.ADD_CLIP, {
    track_id: trackId,
    file_path: filePath,
    start,
    end,
  });
}

/**
 * Move a clip to a new position.
 *
 * Latency strategy: call this on drag-end (deferred commit). During the drag
 * itself, update only local UI state for 60fps feedback. On drop, commit here.
 */
export function moveClip(
  trackId: string,
  clipId: string,
  start: number,
  end: number,
): Promise<CommandResult> {
  return dispatch<MoveClipPayload>(cmd.MOVE_CLIP, {
    track_id: trackId,
    clip_id: clipId,
    start,
    end,
  });
}

export function removeClip(trackId: string, clipId: string): Promise<CommandResult> {
  return dispatch<RemoveClipPayload>(cmd.REMOVE_CLIP, {
    track_id: trackId,
    clip_id: clipId,
  });
}

// ── Project metadata commands ─────────────────────────────────────────────────

export function updateProjectName(name: string): Promise<CommandResult> {
  return dispatch<UpdateProjectNamePayload>(cmd.UPDATE_PROJECT_NAME, { name });
}

export function updateTempo(
  bpm: number,
  numerator: number,
  denominator: number,
): Promise<CommandResult> {
  return dispatch<UpdateTempoPayload>(cmd.UPDATE_TEMPO, { bpm, numerator, denominator });
}

// ── Transport commands ────────────────────────────────────────────────────────
//
// Transport commands are "immediate" — they update the engine synchronously
// and the frontend transport store updates via the engine-event channel.

export function play(): Promise<CommandResult> {
  return dispatch(cmd.PLAY, {});
}

export function stop(): Promise<CommandResult> {
  return dispatch(cmd.STOP, {});
}

export function pause(): Promise<CommandResult> {
  return dispatch(cmd.PAUSE, {});
}

export function seek(position: number): Promise<CommandResult> {
  return dispatch<SeekPayload>(cmd.SEEK, { position });
}

// ── Effect commands ───────────────────────────────────────────────────────────

export function addEffect(trackId: string, kind: string): Promise<CommandResult> {
  return dispatch<AddEffectPayload>(cmd.ADD_EFFECT, { track_id: trackId, kind });
}

export function removeEffect(trackId: string, effectId: string): Promise<CommandResult> {
  return dispatch<RemoveEffectPayload>(cmd.REMOVE_EFFECT, { track_id: trackId, effect_id: effectId });
}

export function updateEffect(
  trackId: string,
  effectId: string,
  changes: { enabled?: boolean; params?: Record<string, unknown> },
): Promise<CommandResult> {
  return dispatch<UpdateEffectPayload>(cmd.UPDATE_EFFECT, {
    track_id: trackId,
    effect_id: effectId,
    ...changes,
  });
}

// ── Modulation commands ───────────────────────────────────────────────────────

export function addModulator(
  trackId: string,
  kind: 'lfo',
  rateHz = 1.0,
): Promise<CommandResult> {
  return dispatch<AddModulatorPayload>(cmd.ADD_MODULATOR, {
    track_id: trackId,
    kind,
    rate_hz: rateHz,
  });
}

export function removeModulator(trackId: string, modulatorId: string): Promise<CommandResult> {
  return dispatch<RemoveModulatorPayload>(cmd.REMOVE_MODULATOR, {
    track_id: trackId,
    modulator_id: modulatorId,
  });
}

export function updateModulator(
  trackId: string,
  modulatorId: string,
  changes: { rate_hz?: number },
): Promise<CommandResult> {
  return dispatch<UpdateModulatorPayload>(cmd.UPDATE_MODULATOR, {
    track_id: trackId,
    modulator_id: modulatorId,
    ...changes,
  });
}

export function addModulationRoute(
  trackId: string,
  modulatorId: string,
  targetKind: 'track_gain' | 'effect_param',
  param: string,
  depth: number,
  effectId?: string,
): Promise<CommandResult> {
  return dispatch<AddModulationRoutePayload>(cmd.ADD_MODULATION_ROUTE, {
    track_id: trackId,
    modulator_id: modulatorId,
    target_kind: targetKind,
    param,
    depth,
    effect_id: effectId,
  });
}

export function removeModulationRoute(trackId: string, routeId: string): Promise<CommandResult> {
  return dispatch<RemoveModulationRoutePayload>(cmd.REMOVE_MODULATION_ROUTE, {
    track_id: trackId,
    route_id: routeId,
  });
}

export function setModulationDepth(
  trackId: string,
  routeId: string,
  depth: number,
): Promise<CommandResult> {
  return dispatch<SetModulationDepthPayload>(cmd.SET_MODULATION_DEPTH, {
    track_id: trackId,
    route_id: routeId,
    depth,
  });
}

// ── MIDI commands ─────────────────────────────────────────────────────────────

export function setMidiNotes(
  trackId: string,
  notes: SetMidiNotesPayload['notes'],
): Promise<CommandResult> {
  return dispatch<SetMidiNotesPayload>(cmd.SET_MIDI_NOTES, {
    track_id: trackId,
    notes,
  });
}

export function addMidiClip(
  trackId: string,
  start: number,
  end: number,
): Promise<CommandResult> {
  return dispatch<AddMidiClipPayload>(cmd.ADD_MIDI_CLIP, { track_id: trackId, start, end });
}

export function removeMidiClip(trackId: string, clipId: string): Promise<CommandResult> {
  return dispatch<RemoveMidiClipPayload>(cmd.REMOVE_MIDI_CLIP, { track_id: trackId, clip_id: clipId });
}

export function moveMidiClip(
  trackId: string,
  clipId: string,
  start: number,
  end: number,
): Promise<CommandResult> {
  return dispatch<MoveMidiClipPayload>(cmd.MOVE_MIDI_CLIP, { track_id: trackId, clip_id: clipId, start, end });
}

export function setClipMidiNotes(
  trackId: string,
  clipId: string,
  notes: SetClipMidiNotesPayload['notes'],
): Promise<CommandResult> {
  return dispatch<SetClipMidiNotesPayload>(cmd.SET_CLIP_MIDI_NOTES, {
    track_id: trackId,
    clip_id: clipId,
    notes,
  });
}

// ── History ───────────────────────────────────────────────────────────────────

/** Undo the last event-sourced project change. Returns the new project snapshot. */
export async function undoCommand(): Promise<unknown> {
  return invoke('undo');
}

/** Redo the next event-sourced project change. Returns the new project snapshot. */
export async function redoCommand(): Promise<unknown> {
  return invoke('redo');
}
