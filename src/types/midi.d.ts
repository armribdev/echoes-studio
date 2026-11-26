export interface MidiNote {
	id: string;
	pitch: number; // 0–127 (MIDI standard, 60 = C4)
	start: number; // en beats (float)
	duration: number; // en beats (float, > 0)
	velocity: number; // 0–127
	/** Courbes de la note (ADR-039). Absent pour une note sans courbe. */
	expressions?: NoteExpressionPoint[];
}

export type NoteExpressionKind = 'pressure' | 'timbre' | 'pitch';

/** Valeur tenue jusqu'au point suivant du même type ; pression et timbre 0..1, pitch en demi-tons (±24). */
export interface NoteExpressionPoint {
	start: number; // en beats, relatif au début de la note
	kind: NoteExpressionKind;
	value: number;
}
