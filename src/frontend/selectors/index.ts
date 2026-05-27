/**
 * Selectors — derive specific UI-relevant slices from ProjectState.
 *
 * Rules:
 * - Selectors are PURE functions (no side effects).
 * - They accept ProjectState (or sub-parts) and return derived data.
 * - No Svelte store logic here — keep this framework-agnostic.
 * - Svelte components use `derived(engineProject, sel)` to consume these.
 *
 * Usage example:
 *   import { getTrack, getClips } from '$frontend/selectors';
 *   const track = derived(engineProject, (p) => getTrack(p, trackId));
 */

import type { ProjectState, TrackState, ClipState } from '../../shared/project/reducer';

// ── Track selectors ────────────────────────────────────────────────────────────

/** Look up a track by ID. Returns undefined if not found. */
export function getTrack(state: ProjectState, trackId: string): TrackState | undefined {
  return state.tracks.find((t) => t.id === trackId);
}

/** All tracks in order. */
export function getTracks(state: ProjectState): TrackState[] {
  return state.tracks;
}

/** Number of tracks. */
export function getTrackCount(state: ProjectState): number {
  return state.tracks.length;
}

// ── Clip selectors ────────────────────────────────────────────────────────────

/** All clips for a given track. Returns [] if track not found. */
export function getClips(state: ProjectState, trackId: string): ClipState[] {
  return state.tracks.find((t) => t.id === trackId)?.clips ?? [];
}

/** Look up a clip by ID across all tracks. */
export function getClip(state: ProjectState, clipId: string): ClipState | undefined {
  for (const track of state.tracks) {
    const clip = track.clips.find((c) => c.id === clipId);
    if (clip) return clip;
  }
  return undefined;
}

/** All clips across all tracks, sorted by start time. */
export function getAllClipsSorted(state: ProjectState): ClipState[] {
  return state.tracks
    .flatMap((t) => t.clips)
    .sort((a, b) => a.start - b.start);
}

/** Clips that overlap a given time range [startSec, endSec). */
export function getClipsInRange(
  state: ProjectState,
  trackId: string,
  startSec: number,
  endSec: number,
): ClipState[] {
  return getClips(state, trackId).filter(
    (c) => c.start < endSec && c.end > startSec,
  );
}

/** The clip that is active at a given playback position. */
export function getActiveClips(
  state: ProjectState,
  positionSec: number,
): ClipState[] {
  return state.tracks
    .flatMap((t) => t.clips)
    .filter((c) => positionSec >= c.start && positionSec < c.end);
}

// ── Transport selectors ───────────────────────────────────────────────────────

/** Current transport state from engine state. */
export function getTransportState(state: ProjectState): {
  bpm: number;
  numerator: number;
  denominator: number;
} {
  return { bpm: state.bpm, numerator: state.numerator, denominator: state.denominator };
}

/** Seconds per beat at the current tempo. */
export function secondsPerBeat(state: ProjectState): number {
  return 60 / state.bpm;
}

/** Seconds per bar at the current tempo. */
export function secondsPerBar(state: ProjectState): number {
  return secondsPerBeat(state) * state.numerator;
}

/** Convert a timeline position in seconds to bar.beat notation. */
export function positionToBarBeat(
  state: ProjectState,
  positionSec: number,
): { bar: number; beat: number } {
  const spb = secondsPerBeat(state);
  const spBar = secondsPerBar(state);
  const bar = Math.floor(positionSec / spBar) + 1;
  const beat = Math.floor((positionSec % spBar) / spb) + 1;
  return { bar, beat };
}

// ── Meter selectors ───────────────────────────────────────────────────────────

/**
 * Track volume as a gain factor (0 – 1).
 * The engine stores volume in the 0–100 range.
 */
export function getTrackGain(state: ProjectState, trackId: string): number {
  return (getTrack(state, trackId)?.volume ?? 0) / 100;
}
