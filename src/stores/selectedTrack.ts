import { writable } from 'svelte/store';
import type { TrackEffect, ProjectModulator, ProjectModulationRoute } from '../types/global';

export interface SelectedTrackInfo {
	trackId: string | null;
	trackName: string;
	trackColor: string;
	instrument: string | null;
	effects: TrackEffect[];
	modulators: ProjectModulator[];
	mod_routes: ProjectModulationRoute[];
}

export const selectedTrack = writable<SelectedTrackInfo>({
	trackId: null,
	trackName: '',
	trackColor: '#4d96ff',
	instrument: null,
	effects: [],
	modulators: [],
	mod_routes: []
});
