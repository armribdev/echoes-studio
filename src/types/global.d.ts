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
	| { kind: 'delay'; params: DelayParams }
	| { kind: 'plugin'; params: PluginEffectParams };

export interface PluginEffectParams {
	plugin_id: string;
	name: string;
	format: string;
	/** `(native_id, normalised value)` pairs. */
	params: [number, number][];
	/** Runs in its own process: a crash does not take Echoes down. */
	sandboxed: boolean;
	/** Track id feeding this plugin's aux (sidechain) input, or null/absent for none. */
	sidechain_source?: string | null;
}

export interface TrackEffect {
	id: string;
	enabled: boolean;
	params: EffectParamsUnion;
}

export type LfoWaveform = 'sine' | 'triangle' | 'saw' | 'square';
export type ModulatorKind = 'lfo' | 'envelope' | 'audio_follower' | 'macro';
export type FollowerMode = 'rms' | 'peak';

/** Flattened `ModulatorParams`: only the fields of `kind` are present. */
export interface ProjectModulator {
	id: string;
	kind: ModulatorKind;
	rate_hz?: number;
	waveform?: LfoWaveform;
	tempo_sync?: boolean;
	/** Beats per LFO cycle when `tempo_sync` is on. */
	beat_division?: number;
	/** Macro value, 0…1. */
	value?: number;
	attack_ms?: number;
	decay_ms?: number;
	/** Envelope sustain level, 0…1. */
	sustain_level?: number;
	release_ms?: number;
	window_ms?: number;
	mode?: FollowerMode;
	/** Follower tap: after this effect, or the track post-FX output when absent. */
	source_effect_id?: string | null;
}

export type ModulationTargetRef =
	| { kind: 'track_gain'; track_id: string }
	| { kind: 'effect_param'; track_id: string; effect_id: string; param: string }
	| { kind: 'plugin_param'; effect_id: string; native_id: number };

export type ModulationTargetKind = ModulationTargetRef['kind'];

export interface ProjectModulationRoute {
	id: string;
	modulator_id: string;
	target: ModulationTargetRef;
	depth: number;
}

export type InterpolationMode = 'linear' | 'step';

export interface AutomationPoint {
	time_s: number;
	/** Normalised parameter value [0..1]. */
	value: number;
}

export interface AutomationClip {
	id: string;
	start_s: number;
	end_s: number;
	points: AutomationPoint[];
}

export interface AutomationLane {
	id: string;
	target: ModulationTargetRef;
	clips: AutomationClip[];
	interpolation: InterpolationMode;
}

export interface MidiNote {
	id: string;
	pitch: number;
	/** Beat position relative to the clip start. */
	start: number;
	duration: number;
	velocity: number;
}

export interface MidiClip {
	id: string;
	/** Position on the timeline in seconds. */
	start: number;
	end: number;
	/** Seconds of content hidden past the left edge (left-trim). Defaults to 0. */
	offset: number;
	notes: MidiNote[];
}

/** An aux output of a multi-out plugin, taken as a track's audio input. */
export interface PluginOutputSource {
	/** Plugin placement: an effect id, or `instrument:{track_id}`. */
	effect_id: string;
	/** Plugin output port, from 1 (0 is the main output). */
	port: number;
}

/** An aux output a loaded plugin declares (`get_plugin_outputs`). */
export interface PluginOutputInfo {
	port: number;
	name: string;
	channel_count: number;
}

export interface Track {
	/** Stable ID used by the event-sourcing system. Added in architecture v2. */
	id: string;
	name: string;
	volume: number;
	pan: number;
	muted: boolean;
	solo: boolean;
	/** Optional UI color override (`#rrggbb`). Absent → stable default from id. */
	color?: string | null;
	/** Plugin id (e.g. `clap:…`) or null for built-in PolySynth. */
	instrument?: string | null;
	/** Plugin output mixed into the track's input, or null/absent for none. */
	input_source?: PluginOutputSource | null;
	clips: Clip[];
	effects: TrackEffect[];
	modulators: ProjectModulator[];
	mod_routes: ProjectModulationRoute[];
	midi_notes: MidiNote[];
	midi_clips: MidiClip[];
	automation_lanes: AutomationLane[];
}

export interface Clip {
	id: string;
	start: number;
	end: number;
	/** Seconds skipped into the source file before playback (left-trim). Defaults to 0. */
	offset: number;
	file_path: string;
	/** Always empty from the engine — PCM stays in Rust. Kept optional for older snapshots. */
	raw_samples?: number[];
	sample_rate: number;
	/** Full source-file duration in seconds (preferred for trim limits). */
	source_duration_sec?: number;
	peaks_64: [number, number][];
	peaks_512: [number, number][];
	peaks_4096: [number, number][];
}

export interface SidePanelLayoutDto {
	open: boolean;
	width: number;
}

export interface BottomPanelLayoutDto {
	open: boolean;
	height: number;
	saved_edit_height: number;
	saved_preview_height: number;
}

export interface ViewModeLayoutDto {
	main: string;
	graph_scope: string;
}

export interface ProjectUiLayout {
	left_panel: SidePanelLayoutDto;
	right_panel: SidePanelLayoutDto;
	bottom_panel: BottomPanelLayoutDto;
	view_mode: ViewModeLayoutDto;
}

export interface Project {
	name: string;
	bpm: number;
	numerator: number;
	denominator: number;
	/** Grille MIDI / arrangement en fractions de noire (0.25 = 1/16). */
	midi_grid_beats: number;
	key_root: string;
	key_mode: string;
	swing_percent: number;
	midi_ppq: number;
	midi_grid_use_triplets: boolean;
	tracks: Track[];
	/** Locators — named points in beats. Absent/empty on older projects. */
	markers?: ArrangementMarker[];
	/** Arrangement sections — named spans in beats. Absent/empty on older projects. */
	regions?: ArrangementRegion[];
	ui_layout?: ProjectUiLayout;
}

/** Named point on the arrangement timeline (beats). */
export interface ArrangementMarker {
	id: string;
	name: string;
	position_beats: number;
	color?: string | null;
}

/** Named span on the arrangement timeline (beats). */
export interface ArrangementRegion {
	id: string;
	name: string;
	start_beats: number;
	end_beats: number;
	color?: string | null;
}
