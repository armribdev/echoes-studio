import { writable } from 'svelte/store';

export const projectName = writable<string>('Sans titre');
export const projectPath = writable<string | null>(null);
