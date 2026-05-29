import { writable } from 'svelte/store';

/** Which editor zone last received a pointer interaction. */
export type KeyboardFocusZone = 'timeline' | 'piano-roll' | 'settings';

function createKeyboardFocusStore() {
	const { subscribe, set } = writable<KeyboardFocusZone>('timeline');

	return {
		subscribe,
		setTimeline: () => set('timeline'),
		setPianoRoll: () => set('piano-roll'),
		setSettings: () => set('settings'),
	};
}

export const keyboardFocus = createKeyboardFocusStore();
