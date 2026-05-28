import { writable } from 'svelte/store';
import type { TrackLevelsMap } from '../shared/protocol/events';

/**
 * Real-time per-track peak levels, updated at ~30 fps by the engine reporter.
 * Values are linear amplitude in [0, 1].  Convert to dBFS for display:
 *   dB = 20 * Math.log10(peak)  (floors at -60 dB)
 */
export const trackLevels = writable<TrackLevelsMap>({});
