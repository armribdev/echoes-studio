import { writable } from 'svelte/store';

/** Which editor zone last received a pointer interaction. */
export type KeyboardFocusZone =
	| 'timeline'
	| 'piano-roll'
	| 'settings'
	| 'library-results'
	| 'library-search';

function createKeyboardFocusStore() {
	const { subscribe, update } = writable<KeyboardFocusZone>('timeline');

	const setZone = (zone: KeyboardFocusZone) => {
		update((current) => (current === zone ? current : zone));
	};

	return {
		subscribe,
		setTimeline: () => setZone('timeline'),
		setPianoRoll: () => setZone('piano-roll'),
		setSettings: () => setZone('settings'),
		setLibraryResults: () => setZone('library-results'),
		setLibrarySearch: () => setZone('library-search'),
	};
}

export const keyboardFocus = createKeyboardFocusStore();
