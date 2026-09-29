import { writable } from 'svelte/store';

/** True while the armed track is recording hardware MIDI notes. Session-only UI state. */
export const isMidiRecording = writable(false);
