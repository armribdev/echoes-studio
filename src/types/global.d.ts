export interface EqParams {
	low_gain_db: number;
	low_freq: number;
	mid_gain_db: number;
	mid_freq: number;
	mid_q: number;
	high_gain_db: number;
	high_freq: number;
}

export interface CompressorParams {
	threshold_db: number;
	ratio: number;
	attack_ms: number;
	release_ms: number;
	makeup_db: number;
	knee_db: number;
}

export interface ReverbParams {
	room_size: number;
	damping: number;
	wet: number;
	dry: number;
	width: number;
}

export interface DelayParams {
	delay_ms: number;
	feedback: number;
	wet: number;
	dry: number;
	ping_pong: boolean;
}

export type EffectKind = 'eq' | 'compressor' | 'reverb' | 'delay';

export type EffectParamsUnion =
	| { kind: 'eq'; params: EqParams }
	| { kind: 'compressor'; params: CompressorParams }
	| { kind: 'reverb'; params: ReverbParams }
	| { kind: 'delay'; params: DelayParams };

export interface TrackEffect {
	id: string;
	enabled: boolean;
	params: EffectParamsUnion;
}

export interface ProjectModulator {
	id: string;
	kind: string;
	rate_hz?: number;
	waveform?: string;
}

export type ModulationTargetRef =
	| { kind: 'track_gain'; track_id: string }
	| { kind: 'effect_param'; track_id: string; effect_id: string; param: string };

export interface ProjectModulationRoute {
	id: string;
	modulator_id: string;
	target: ModulationTargetRef;
	depth: number;
}

export interface MidiNote {
	id: string;
	pitch: number;
	start: number;
	duration: number;
	velocity: number;
}

export interface Track {
	/** Stable ID used by the event-sourcing system. Added in architecture v2. */
	id: string;
	name: string;
	volume: number;
	pan: number;
	muted: boolean;
	solo: boolean;
	clips: Clip[];
	effects: TrackEffect[];
	modulators: ProjectModulator[];
	mod_routes: ProjectModulationRoute[];
	midi_notes: MidiNote[];
}

export interface Clip {
	id: string;
	start: number;
	end: number;
	file_path: string;
	raw_samples: number[];
	sample_rate: number;
	peaks_64: [number, number][];
	peaks_512: [number, number][];
	peaks_4096: [number, number][];
}

export interface Project {
	name: string;
	bpm: number;
	numerator: number;
	denominator: number;
	tracks: Track[];
}
