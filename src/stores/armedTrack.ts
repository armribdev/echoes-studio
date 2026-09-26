import { writable } from 'svelte/store';

/**
 * The track currently armed for hardware MIDI input (ADR-022), or `null`.
 * Session-only UI state, mirroring `selectedTrack.ts` — not project data.
 */
export const armedTrackId = writable<string | null>(null);
