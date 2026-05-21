import { writable } from 'svelte/store';

interface TransportState {
	isPlaying: boolean;
	position: number; // seconds
}

export const transport = writable<TransportState>({
	isPlaying: false,
	position: 0
});
