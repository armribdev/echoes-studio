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

import { get } from 'svelte/store';
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
  type SetLoopPayload,
  type SetModulationDepthPayload,
  type AddModulationRoutePayload,
  type AddAutomationLanePayload,
  type RemoveAutomationLanePayload,
  type UpdateAutomationClipPayload,
  type AddModulatorPayload,
  type SetTrackInputPayload,
  type SetTrackInstrumentPayload,
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
  type AddMarkerPayload,
  type UpdateMarkerPayload,
  type RemoveMarkerPayload,
  type AddRegionPayload,
  type UpdateRegionPayload,
  type RemoveRegionPayload,
} from '../../shared/protocol/commands';
import type { ProjectMusicalFields } from '../../lib/projectMusical';
import {
  DEFAULT_KEY_MODE,
  DEFAULT_KEY_ROOT,
  DEFAULT_MIDI_PPQ,
  DEFAULT_SWING_PERCENT,
} from '../../lib/projectMusical';
import { normalizeMidiGridBeats } from '../../lib/midiGrid';
import { engineProject } from '../store/engineStore';
import { tempo } from '../../stores/tempo';
import { midiGridBeats } from '../../stores/project';

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

export function createTrack(name: string, instrument: string | null = null): Promise<CommandResult> {
  return dispatch<CreateTrackPayload>(cmd.CREATE_TRACK, { name, instrument });
}

export function deleteTrack(trackId: string): Promise<CommandResult> {
  return dispatch<DeleteTrackPayload>(cmd.DELETE_TRACK, { track_id: trackId });
}

let mixLiveFlight: Promise<void> | null = null;
let mixLivePending: { trackId: string; volume?: number; pan?: number } | null = null;
let mixLiveTimer: ReturnType<typeof setTimeout> | null = null;

/** Stream a fader or pan move to the audio thread. Persisted later by `updateTrack`. */
export function setTrackMixLive(trackId: string, volume?: number, pan?: number): void {
  mixLivePending = { trackId, volume, pan };
  if (mixLiveFlight || mixLiveTimer) return;
  mixLiveTimer = setTimeout(() => {
    mixLiveTimer = null;
    const pump = (): Promise<void> => {
      const next = mixLivePending;
      mixLivePending = null;
      if (!next) {
        mixLiveFlight = null;
        return Promise.resolve();
      }
      mixLiveFlight = invoke('set_track_mix', {
        trackId: next.trackId,
        volume: next.volume ?? null,
        pan: next.pan ?? null,
      })
        .then(() => pump())
        .catch((err) => {
          console.error('setTrackMixLive failed:', err);
          return pump();
        });
      return mixLiveFlight;
    };
    void pump();
  }, 32);
}

export function updateTrack(
  trackId: string,
  changes: { name?: string; volume?: number; pan?: number; color?: string },
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

export function setTrackInstrument(
  trackId: string,
  instrument: string | null,
): Promise<CommandResult> {
  return dispatch<SetTrackInstrumentPayload>(cmd.SET_TRACK_INSTRUMENT, {
    track_id: trackId,
    instrument,
  });
}

export function setTrackInput(
  trackId: string,
  inputSource: SetTrackInputPayload['input_source'],
): Promise<CommandResult> {
  return dispatch<SetTrackInputPayload>(cmd.SET_TRACK_INPUT, {
    track_id: trackId,
    input_source: inputSource,
  });
}

// ── Clip commands ─────────────────────────────────────────────────────────────

export function addClip(
  trackId: string,
  filePath: string,
  start: number,
  end: number,
  offset?: number,
): Promise<CommandResult> {
  return dispatch<AddClipPayload>(cmd.ADD_CLIP, {
    track_id: trackId,
    file_path: filePath,
    start,
    end,
    offset,
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
  offset?: number,
  targetTrackId?: string,
): Promise<CommandResult> {
  return dispatch<MoveClipPayload>(cmd.MOVE_CLIP, {
    track_id: trackId,
    clip_id: clipId,
    start,
    end,
    offset,
    target_track_id: targetTrackId,
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

export function currentProjectMusical(): ProjectMusicalFields {
  const t = get(tempo);
  const p = get(engineProject);
  return {
    bpm: t.bpm,
    numerator: t.numerator,
    denominator: t.denominator,
    midi_grid_beats: normalizeMidiGridBeats(get(midiGridBeats)),
    key_root: p.key_root ?? DEFAULT_KEY_ROOT,
    key_mode: p.key_mode ?? DEFAULT_KEY_MODE,
    swing_percent: p.swing_percent ?? DEFAULT_SWING_PERCENT,
    midi_ppq: p.midi_ppq ?? DEFAULT_MIDI_PPQ,
    midi_grid_use_triplets: p.midi_grid_use_triplets ?? false,
  };
}

export function updateProjectMusical(
  fields: ProjectMusicalFields,
): Promise<CommandResult> {
  return dispatch<UpdateTempoPayload>(cmd.UPDATE_TEMPO, {
    ...fields,
    midi_grid_beats: normalizeMidiGridBeats(fields.midi_grid_beats),
    swing_percent: Math.min(100, Math.max(0, Math.round(fields.swing_percent))),
  });
}

export function updateTempo(
  bpm: number,
  numerator: number,
  denominator: number,
  midi_grid_beats?: number,
): Promise<CommandResult> {
  const cur = currentProjectMusical();
  return updateProjectMusical({
    ...cur,
    bpm,
    numerator,
    denominator,
    midi_grid_beats: midi_grid_beats ?? cur.midi_grid_beats,
  });
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

export function setLoop(enabled: boolean, start: number, end: number): Promise<CommandResult> {
  return dispatch<SetLoopPayload>(cmd.SET_LOOP, { enabled, start, end });
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
  changes: {
    enabled?: boolean;
    params?: Record<string, unknown>;
    sandboxed?: boolean;
    /** Track id sidechaining into this plugin's aux input; null clears it. */
    sidechain_source?: string | null;
  },
): Promise<CommandResult> {
  return dispatch<UpdateEffectPayload>(cmd.UPDATE_EFFECT, {
    track_id: trackId,
    effect_id: effectId,
    ...changes,
  });
}

// ── Modulation commands ───────────────────────────────────────────────────────

export function addModulator(trackId: string, kind: AddModulatorPayload['kind']): Promise<CommandResult> {
  return dispatch<AddModulatorPayload>(cmd.ADD_MODULATOR, { track_id: trackId, kind });
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
  changes: Omit<UpdateModulatorPayload, 'track_id' | 'modulator_id'>,
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
  targetKind: 'track_gain' | 'effect_param' | 'plugin_param',
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

// ── Automation commands ───────────────────────────────────────────────────────

export function addAutomationLane(
  trackId: string,
  targetKind: AddAutomationLanePayload['target_kind'],
  param: string,
  effectId?: string,
): Promise<CommandResult> {
  return dispatch<AddAutomationLanePayload>(cmd.ADD_AUTOMATION_LANE, {
    track_id: trackId,
    target_kind: targetKind,
    param,
    effect_id: effectId,
  });
}

export function removeAutomationLane(trackId: string, laneId: string): Promise<CommandResult> {
  return dispatch<RemoveAutomationLanePayload>(cmd.REMOVE_AUTOMATION_LANE, {
    track_id: trackId,
    lane_id: laneId,
  });
}

export function updateAutomationClip(
  trackId: string,
  laneId: string,
  clip: UpdateAutomationClipPayload['clip'],
): Promise<CommandResult> {
  return dispatch<UpdateAutomationClipPayload>(cmd.UPDATE_AUTOMATION_CLIP, {
    track_id: trackId,
    lane_id: laneId,
    clip,
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
  options?: { offset?: number; notes?: AddMidiClipPayload['notes'] },
): Promise<CommandResult> {
  return dispatch<AddMidiClipPayload>(cmd.ADD_MIDI_CLIP, {
    track_id: trackId,
    start,
    end,
    offset: options?.offset,
    notes: options?.notes,
  });
}

export function removeMidiClip(trackId: string, clipId: string): Promise<CommandResult> {
  return dispatch<RemoveMidiClipPayload>(cmd.REMOVE_MIDI_CLIP, { track_id: trackId, clip_id: clipId });
}

export function moveMidiClip(
  trackId: string,
  clipId: string,
  start: number,
  end: number,
  offset?: number,
  targetTrackId?: string,
): Promise<CommandResult> {
  return dispatch<MoveMidiClipPayload>(cmd.MOVE_MIDI_CLIP, {
    track_id: trackId,
    clip_id: clipId,
    start,
    end,
    offset,
    target_track_id: targetTrackId,
  });
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

// ── Arrangement markers / regions ─────────────────────────────────────────────

export function addMarker(
  positionBeats: number,
  name?: string,
  color?: string | null,
): Promise<CommandResult> {
  return dispatch<AddMarkerPayload>(cmd.ADD_MARKER, {
    position_beats: positionBeats,
    name,
    color,
  });
}

export function updateMarker(
  id: string,
  patch: { name?: string; position_beats?: number; color?: string | null },
): Promise<CommandResult> {
  return dispatch<UpdateMarkerPayload>(cmd.UPDATE_MARKER, { id, ...patch });
}

export function removeMarker(id: string): Promise<CommandResult> {
  return dispatch<RemoveMarkerPayload>(cmd.REMOVE_MARKER, { id });
}

export function addRegion(
  startBeats: number,
  endBeats: number,
  name?: string,
  color?: string | null,
): Promise<CommandResult> {
  return dispatch<AddRegionPayload>(cmd.ADD_REGION, {
    start_beats: startBeats,
    end_beats: endBeats,
    name,
    color,
  });
}

export function updateRegion(
  id: string,
  patch: {
    name?: string;
    start_beats?: number;
    end_beats?: number;
    color?: string | null;
  },
): Promise<CommandResult> {
  return dispatch<UpdateRegionPayload>(cmd.UPDATE_REGION, { id, ...patch });
}

export function removeRegion(id: string): Promise<CommandResult> {
  return dispatch<RemoveRegionPayload>(cmd.REMOVE_REGION, { id });
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
