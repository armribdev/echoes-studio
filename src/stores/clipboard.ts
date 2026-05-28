import { writable } from 'svelte/store';

export interface ClipboardEntry {
  trackId: string;
  filePath: string;
  durationSec: number;
  /** Seconds from the earliest clip in the copied selection. */
  relativeStart: number;
}

export const clipboard = writable<ClipboardEntry[]>([]);
