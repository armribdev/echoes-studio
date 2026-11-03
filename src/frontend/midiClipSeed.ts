import type { MidiClip, MidiContent, MidiControlPoint, MidiNote } from '../types/global';

export interface MidiClipSeed {
	offset: number;
	notes: MidiNote[];
	channelControls: MidiControlPoint[];
}

/** What `ADD_MIDI_CLIP` needs to make a copy of a clip's content (paste, duplicate, repeat). */
export function midiClipSeed(clip: Pick<MidiClip, 'offset'>, content: MidiContent): MidiClipSeed {
	return {
		offset: clip.offset ?? 0,
		notes: content.notes.map((note) => ({ ...note })),
		channelControls: content.channel_controls.map((point) => ({ ...point }))
	};
}
