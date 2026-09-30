import type { MidiNote } from '../types/global';
import type { RecordedNote } from '../lib/midiBackend';
import type { MidiRecordingPreview } from '../stores/midiRecording';

export interface PreviewClip {
	start: number;
	end: number;
	notes: MidiNote[];
}

/**
 * The clip as it would be if the recording ended at `playhead`: same shape as
 * the finished clip, so the handoff to the real clip does not move anything.
 */
export function buildPreviewClip(
	preview: MidiRecordingPreview,
	playhead: number,
	bpm: number
): PreviewClip {
	if (preview.finished) {
		const { start, end, notes } = preview.finished;
		return { start, end, notes };
	}

	const start = Math.min(preview.start, ...preview.events.map((event) => event.position));
	const beatsPerSecond = bpm / 60;
	const held = new Map<number, RecordedNote>();
	const notes: MidiNote[] = [];
	let end = playhead;

	const closeNote = (noteOn: RecordedNote, position: number) => {
		const noteEnd = Math.max(position, noteOn.position);
		end = Math.max(end, noteEnd);
		notes.push({
			id: `recording-${notes.length}`,
			pitch: noteOn.pitch,
			velocity: noteOn.velocity,
			start: (noteOn.position - start) * beatsPerSecond,
			duration: (noteEnd - noteOn.position) * beatsPerSecond
		});
	};

	for (const event of preview.events) {
		const previous = held.get(event.pitch);
		if (previous) closeNote(previous, event.position);
		if (event.noteOn) held.set(event.pitch, event);
		else held.delete(event.pitch);
	}
	for (const noteOn of held.values()) closeNote(noteOn, playhead);

	return { start, end: Math.max(end, start), notes };
}
