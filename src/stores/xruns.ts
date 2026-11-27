import { writable } from 'svelte/store';

/** Cumulative audio underruns of the current stream session, pushed by the engine reporter. */
export const xrunCount = writable(0);
