import { writable } from 'svelte/store';

/** True while the preferences panel is capturing a new key chord. */
export const shortcutRecording = writable(false);
