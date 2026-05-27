import { writable } from 'svelte/store';

/**
 * Incremented each time the user requests a mixdown export (Ctrl+Shift+E or
 * the toolbar button). Components that own the export logic subscribe to this
 * and run the export when the value changes.
 */
export const exportTrigger = writable(0);
