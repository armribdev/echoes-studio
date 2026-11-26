import { DEFAULT_TRACK_COLOR } from '../frontend/trackColor';
import { writable } from 'svelte/store';
import type {
	ArpeggiatorSettings,
	NoteEffectSettings,
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
	note_effects: NoteEffectSettings | null;
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
	note_effects: null,
	effects: [],
	modulators: [],
	mod_routes: []
});
