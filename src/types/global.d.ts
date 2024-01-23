export interface Track {
	name: string;
	volume: number;
	clips: Clip[];
}

export interface Clip {
    start: number,
    end: number,
}