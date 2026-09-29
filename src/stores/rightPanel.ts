import { writable } from 'svelte/store';

export const LIBRARY_PANEL_DEFAULT_WIDTH = 360;
export const LIBRARY_PANEL_MIN_WIDTH = 240;
export const LIBRARY_PANEL_MAX_WIDTH = 560;

interface RightPanelState {
	open: boolean;
	width: number;
}

function clampLibraryWidth(w: number): number {
	return Math.max(LIBRARY_PANEL_MIN_WIDTH, Math.min(LIBRARY_PANEL_MAX_WIDTH, w));
}

function createRightPanelStore() {
	const { subscribe, update } = writable<RightPanelState>({
		open: false,
		width: LIBRARY_PANEL_DEFAULT_WIDTH,
	});

	return {
		subscribe,
		hydrate(partial: Partial<RightPanelState>) {
			update((s) => ({
				...s,
				...partial,
				width:
					partial.width !== undefined
						? clampLibraryWidth(partial.width)
						: s.width,
			}));
		},
		toggle() {
			update((s) => ({ ...s, open: !s.open }));
			import('../lib/projectUiLayout').then((m) => m.persistProjectUiLayout());
		},
		setWidth(w: number) {
			update((s) => ({ ...s, width: clampLibraryWidth(w) }));
		},
	};
}

export const rightPanel = createRightPanelStore();
