import { writable } from 'svelte/store';

export const TRACK_INSPECTOR_DEFAULT_WIDTH = 168;
export const TRACK_INSPECTOR_MIN_WIDTH = 140;
export const TRACK_INSPECTOR_MAX_WIDTH = 280;

interface LeftPanelState {
	open: boolean;
	width: number;
}

function clampTrackInspectorWidth(w: number): number {
	return Math.max(
		TRACK_INSPECTOR_MIN_WIDTH,
		Math.min(TRACK_INSPECTOR_MAX_WIDTH, w),
	);
}

function createLeftPanelStore() {
	const { subscribe, update } = writable<LeftPanelState>({
		open: false,
		width: TRACK_INSPECTOR_DEFAULT_WIDTH,
	});

	return {
		subscribe,
		hydrate(partial: Partial<LeftPanelState>) {
			update((s) => ({
				...s,
				...partial,
				width:
					partial.width !== undefined
						? clampTrackInspectorWidth(partial.width)
						: s.width,
			}));
		},
		toggle() {
			update((s) => ({ ...s, open: !s.open }));
			import('../lib/projectUiLayout').then((m) => m.persistProjectUiLayout());
		},
		open() {
			let changed = false;
			update((s) => {
				if (s.open) return s;
				changed = true;
				return { ...s, open: true };
			});
			if (changed) {
				import('../lib/projectUiLayout').then((m) => m.persistProjectUiLayout());
			}
		},
		setWidth(w: number) {
			update((s) => ({ ...s, width: clampTrackInspectorWidth(w) }));
		},
	};
}

export const leftPanel = createLeftPanelStore();
