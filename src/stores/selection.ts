import { writable } from 'svelte/store';

export const selection = writable<Set<string>>(new Set());
