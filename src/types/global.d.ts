export interface Track {
	name: string;
	volume: number;
	clips: Clip[];
}

export interface Clip {
	id: string;
	start: number;
	end: number;
	raw_samples: number[];
	sample_rate: number;
	peaks_64: [number, number][];
	peaks_512: [number, number][];
	peaks_4096: [number, number][];
}
