import type { MidiNote } from '../../types/global';

export const NOTE_H = 16;
export const TOTAL_PITCHES = 128;
export const RESIZE_ZONE = 8;

export function snapBeat(beat: number, quantize: number): number {
	return Math.floor(beat / quantize + 1e-9) * quantize;
}

export function pitchToYEdit(pitch: number): number {
	return (TOTAL_PITCHES - 1 - pitch) * NOTE_H;
}

export function pitchToYPreview(
	pitch: number,
	previewPitchMax: number,
	previewRowH: number,
): number {
	return (previewPitchMax - pitch) * previewRowH;
}

export function beatFromGridX(rawX: number, zoom: number): number {
	return rawX / zoom;
}

export function pitchFromGridY(rawY: number, rowH: number): number {
	return TOTAL_PITCHES - 1 - Math.floor(rawY / rowH);
}

export function hitTestNote(
	notes: MidiNote[],
	beat: number,
	pitch: number,
): MidiNote | undefined {
	return notes.find(
		(n) => n.pitch === pitch && beat >= n.start && beat < n.start + n.duration,
	);
}

export function isResizeHandle(note: MidiNote, beatX: number, zoom: number): boolean {
	const endPx = (note.start + note.duration) * zoom;
	const pxX = beatX * zoom;
	return pxX >= endPx - RESIZE_ZONE && pxX <= endPx + 2;
}

export function contentEndBeats(notes: MidiNote[], clipDurationBeats: number): number {
	let end = clipDurationBeats;
	for (const n of notes) {
		end = Math.max(end, n.start + n.duration);
	}
	return end;
}

export function visibleBeatsForEdit(
	notes: MidiNote[],
	clipDurationBeats: number,
	margin = 8,
	minBeats = 16,
): number {
	return Math.max(minBeats, contentEndBeats(notes, clipDurationBeats) + margin);
}
