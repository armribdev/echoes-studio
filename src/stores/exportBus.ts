import { writable } from 'svelte/store';

/**
 * Incremented each time the user requests a WAV mixdown export (Ctrl+Shift+E
 * or the toolbar button). Components that own the export logic subscribe to
 * this and run the export when the value changes.
 */
export const exportTrigger = writable(0);

/**
 * Incremented each time the user requests an MP3 mixdown export (Ctrl+Shift+M
 * or the toolbar button). Components that own the export logic subscribe to
 * this and run the export when the value changes.
 */
export const exportMp3Trigger = writable(0);
