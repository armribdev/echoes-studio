import { writable } from 'svelte/store';
import type { LibraryOpenContext } from './types';

export interface LibraryPopupState {
	open: boolean;
	context: LibraryOpenContext;
	/** Target track; `null` means create a track on insert (empty project). */
	trackId: string | null;
	anchorX: number;
	anchorY: number;
}

const INITIAL: LibraryPopupState = {
	open: false,
	context: 'playhead_insert',
	trackId: null,
	anchorX: 0,
	anchorY: 0,
};

function createLibraryPopupStore() {
	const { subscribe, set, update } = writable<LibraryPopupState>(INITIAL);

	return {
		subscribe,
		open(opts: {
			context: LibraryOpenContext;
			trackId?: string | null;
			anchorX: number;
			anchorY: number;
		}) {
			set({
				open: true,
				context: opts.context,
				trackId: opts.trackId ?? null,
				anchorX: opts.anchorX,
				anchorY: opts.anchorY,
			});
		},
		close() {
			update((s) => (s.open ? { ...s, open: false } : s));
		},
	};
}

export const libraryPopup = createLibraryPopupStore();
