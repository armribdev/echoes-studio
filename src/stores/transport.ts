import { writable } from 'svelte/store';

interface TransportState {
	isPlaying: boolean;
	position: number; // seconds
	isScrubbing: boolean;
}

export const transport = writable<TransportState>({
	isPlaying: false,
	position: 0,
	isScrubbing: false
});
