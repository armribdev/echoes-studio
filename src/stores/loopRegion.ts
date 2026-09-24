import { writable } from 'svelte/store';

/** Playback loop in musical beats. The engine receives the same region in seconds. */
export interface LoopRegion {
	enabled: boolean;
	startBeat: number;
	endBeat: number;
}

const MIN_BEATS = 1;

function clampRegion(startBeat: number, endBeat: number): Pick<LoopRegion, 'startBeat' | 'endBeat'> {
	const start = Math.max(0, startBeat);
	const end = Math.max(start + MIN_BEATS, endBeat);
	return { startBeat: start, endBeat: end };
}

function createLoopRegion() {
	const { subscribe, update, set } = writable<LoopRegion>({
		enabled: false,
		startBeat: 0,
		endBeat: 16,
	});

	return {
		subscribe,
		toggle: () => update((region) => ({ ...region, enabled: !region.enabled })),
		setEnabled: (enabled: boolean) => update((region) => ({ ...region, enabled })),
		setBeats: (startBeat: number, endBeat: number) =>
			update((region) => ({ ...region, ...clampRegion(startBeat, endBeat) })),
		reset: () => set({ enabled: false, startBeat: 0, endBeat: 16 }),
	};
}

export const loopRegion = createLoopRegion();
