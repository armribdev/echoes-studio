export interface MidiNote {
	id: string;
	pitch: number;    // 0–127 (MIDI standard, 60 = C4)
	start: number;    // en beats (float)
	duration: number; // en beats (float, > 0)
	velocity: number; // 0–127
}
