import { DEFAULT_TRACK_COLOR } from '../frontend/trackColor';
import { writable } from 'svelte/store';
import type {
	ArpeggiatorSettings,
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
	arpeggiator: ArpeggiatorSettings | null;
	effects: TrackEffect[];
	modulators: ProjectModulator[];
	mod_routes: ProjectModulationRoute[];
}

export const selectedTrack = writable<SelectedTrackInfo>({
	trackId: null,
	trackName: '',
	trackColor: DEFAULT_TRACK_COLOR,
	instrument: null,
	sampler: null,
	arpeggiator: null,
	effects: [],
	modulators: [],
	mod_routes: []
});
