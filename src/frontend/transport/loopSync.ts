import { derived } from 'svelte/store';
import { setLoop } from '../commands/dispatcher';
import { loopRegion } from '../../stores/loopRegion';
import { tempo } from '../../stores/tempo';

/** Push the musical loop to the engine whenever the region or the tempo changes. */
export function connectLoopSync(): () => void {
	let lastSent = '';
	return derived([loopRegion, tempo], ([$loop, $tempo]) => ({
		enabled: $loop.enabled,
		start: ($loop.startBeat * 60) / $tempo.bpm,
		end: ($loop.endBeat * 60) / $tempo.bpm
	})).subscribe((region) => {
		const key = `${region.enabled}:${region.start}:${region.end}`;
		if (key === lastSent) return;
		lastSent = key;
		setLoop(region.enabled, region.start, region.end).catch(console.error);
	});
}
