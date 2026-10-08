import { writable } from 'svelte/store';
import type {
	TrackEffect,
	ProjectModulator,
	ProjectModulationRoute,
	SamplerKit
} from '../types/global';

export interface SelectedTrackInfo {
	trackId: string | null;
	trackName: string;
	trackColor: string;
	instrument: string | null;
	sampler: SamplerKit | null;
	effects: TrackEffect[];
	modulators: ProjectModulator[];
	mod_routes: ProjectModulationRoute[];
}

export const selectedTrack = writable<SelectedTrackInfo>({
	trackId: null,
	trackName: '',
	trackColor: '#4d96ff',
	instrument: null,
	sampler: null,
	effects: [],
	modulators: [],
	mod_routes: []
});
