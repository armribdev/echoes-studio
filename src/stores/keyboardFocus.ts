import { writable } from 'svelte/store';

/** Which editor zone last received a pointer interaction. */
export type KeyboardFocusZone = 'timeline' | 'piano-roll' | 'settings';

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
	};
}

export const keyboardFocus = createKeyboardFocusStore();
