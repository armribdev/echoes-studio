import type { SamplerPad } from '../../types/global';

const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];

/** Middle C (60) = C4, as in the piano roll. */
export function noteName(pitch: number): string {
	return `${NOTE_NAMES[pitch % 12]}${Math.floor(pitch / 12) - 1}`;
}

/** File name without folder or extension, or the trigger note for an empty pad. */
export function padLabel(pad: SamplerPad): string {
	if (!pad.sample_path) return noteName(pad.note);
	const file = pad.sample_path.split(/[\\/]/).pop() ?? pad.sample_path;
	return file.replace(/\.[^.]+$/, '');
}

/** Pad indices laid out drum-machine style: pad 1 bottom-left, rows of four. */
export function padRows(count: number): number[][] {
	const rows: number[][] = [];
	for (let start = 0; start < count; start += 4) {
		rows.unshift(Array.from({ length: Math.min(4, count - start) }, (_, i) => start + i));
	}
	return rows;
}

export interface PadDropTarget {
	trackId: string;
	pad: number;
}

/** The sampler pad under a screen point, for file and library drops. */
export function padDropTargetAt(clientX: number, clientY: number): PadDropTarget | null {
	const element = document
		.elementFromPoint(clientX, clientY)
		?.closest<HTMLElement>('[data-sampler-pad]');
	const trackId = element?.dataset.trackId;
	const pad = Number(element?.dataset.samplerPad);
	if (!trackId || !Number.isInteger(pad)) return null;
	return { trackId, pad };
}
