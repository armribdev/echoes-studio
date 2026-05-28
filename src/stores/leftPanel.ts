import { writable } from 'svelte/store';

interface LeftPanelState {
	open: boolean;
	width: number;
}

function createLeftPanelStore() {
	const { subscribe, update } = writable<LeftPanelState>({
		open: true,
		width: 240,
	});

	return {
		subscribe,
		toggle() {
			update((s) => ({ ...s, open: !s.open }));
		},
		setWidth(w: number) {
			update((s) => ({ ...s, width: Math.max(180, Math.min(400, w)) }));
		},
	};
}

export const leftPanel = createLeftPanelStore();
