import { writable } from 'svelte/store';

interface BottomPanelState {
	open: boolean;
	height: number;
}

function createBottomPanelStore() {
	const { subscribe, update } = writable<BottomPanelState>({
		open: false,
		height: 320,
	});

	return {
		subscribe,
		openTab(_tab: 'pianoroll') {
			update((s) => ({ ...s, open: !s.open }));
		},
		toggle() {
			update((s) => ({ ...s, open: !s.open }));
		},
		close() {
			update((s) => ({ ...s, open: false }));
		},
		setHeight(h: number) {
			update((s) => ({ ...s, height: Math.max(160, Math.min(600, h)) }));
		},
	};
}

export const bottomPanel = createBottomPanelStore();
