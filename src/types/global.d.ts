export interface Track {
	name: string;
	volume: number;
	clips: Clip[];
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
