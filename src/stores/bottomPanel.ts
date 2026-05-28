import { writable } from 'svelte/store';

export type BottomPanelTab = 'pianoroll';

interface BottomPanelState {
	open: boolean;
	activeTab: BottomPanelTab | null;
	height: number;
}

function createBottomPanelStore() {
	const { subscribe, update } = writable<BottomPanelState>({
		open: false,
		activeTab: null,
		height: 320,
	});

	return {
		subscribe,
		openTab(tab: BottomPanelTab) {
			update((s) => ({ ...s, open: true, activeTab: tab }));
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
