import { writable } from 'svelte/store';

interface RightPanelState {
	open: boolean;
	width: number;
}

function createRightPanelStore() {
	const { subscribe, update } = writable<RightPanelState>({
		open: false,
		width: 260,
	});

	return {
		subscribe,
		toggle() {
			update((s) => ({ ...s, open: !s.open }));
		},
		setWidth(w: number) {
			update((s) => ({ ...s, width: Math.max(200, Math.min(480, w)) }));
		},
	};
}

export const rightPanel = createRightPanelStore();
