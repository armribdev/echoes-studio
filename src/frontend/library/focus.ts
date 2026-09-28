import { writable } from 'svelte/store';

/** Increment to request focus on the library search field (panel opens if needed). */
export const libraryFocusSearchTick = writable(0);

export function requestLibrarySearchFocus() {
	libraryFocusSearchTick.update((n) => n + 1);
}
