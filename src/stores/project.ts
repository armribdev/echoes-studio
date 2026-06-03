import { writable } from 'svelte/store';
import { DEFAULT_MIDI_GRID_BEATS } from '../lib/midiGrid';

export const projectName = writable<string>('Sans titre');
export const projectPath = writable<string | null>(null);
export const projectSettingsPanelOpen = writable(false);
export const midiGridBeats = writable<number>(DEFAULT_MIDI_GRID_BEATS);
