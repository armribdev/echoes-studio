import type { MidiNote } from '../../types/global';
import type { PianoRollMode } from '../../stores/pianoRoll';
import { NOTE_H, TOTAL_PITCHES, hitTestNote, snapBeat } from './coords';
import { hexAlpha, lighten, roundRect } from './colorUtils';

const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];

export interface NotesDrawParams {
	ctx: CanvasRenderingContext2D;
	width: number;
	height: number;
	editMode: boolean;
	zoom: number;
	quantize: number;
	mode: PianoRollMode;
	trackColor: string;
	notes: MidiNote[];
	selectedNoteIds: Set<string>;
	previewPitchMin: number;
	previewPitchMax: number;
	previewRowH: number;
	hoverBeat: number;
	hoverPitch: number;
	dragType: string;
	showSelbox: boolean;
	selbox?: { x0: number; y0: number; x1: number; y1: number };
	clearBackground?: boolean;
}

function pitchToY(p: NotesDrawParams, pitch: number): number {
	if (p.editMode) return (TOTAL_PITCHES - 1 - pitch) * NOTE_H;
	return (p.previewPitchMax - pitch) * p.previewRowH;
}

function noteRowH(p: NotesDrawParams): number {
	return p.editMode ? NOTE_H : p.previewRowH;
}

/** Notes, ghost, boîte de sélection. */
export function drawNotesLayer(p: NotesDrawParams): void {
	const { ctx, width: W, height: H } = p;
	if (p.clearBackground !== false) {
		ctx.clearRect(0, 0, W, H);
	}

	const rowH = noteRowH(p);

	if (
		p.editMode &&
		p.mode === 'draw' &&
		p.dragType === 'none' &&
		p.hoverPitch >= 0 &&
		p.hoverBeat >= 0 &&
		!hitTestNote(p.notes, p.hoverBeat, p.hoverPitch)
	) {
		const gBeat = snapBeat(p.hoverBeat, p.quantize);
		const gx = gBeat * p.zoom;
		const gy = pitchToY(p, p.hoverPitch);
		const gw = Math.max(2, p.quantize * p.zoom - 1);
		ctx.fillStyle = hexAlpha(p.trackColor, 0.28);
		roundRect(ctx, gx, gy + 1, gw, rowH - 2, 3);
		ctx.fill();
		ctx.strokeStyle = hexAlpha(p.trackColor, 0.55);
		ctx.lineWidth = 1;
		roundRect(ctx, gx + 0.5, gy + 1.5, gw - 1, rowH - 3, 3);
		ctx.stroke();
	}

	ctx.font = '9px Inter, system-ui, sans-serif';
	ctx.textBaseline = 'middle';

	for (const note of p.notes) {
		if (!p.editMode && (note.pitch < p.previewPitchMin || note.pitch > p.previewPitchMax)) {
			continue;
		}
		const x = note.start * p.zoom;
		const y = pitchToY(p, note.pitch);
		const w = Math.max(2, note.duration * p.zoom - 1);
		const selected = p.selectedNoteIds.has(note.id);
		const velRatio = note.velocity / 127;
		const alpha = selected ? 1 : 0.42 + velRatio * 0.52;
		const base = selected ? lighten(p.trackColor, 0.22) : p.trackColor;

		ctx.fillStyle = hexAlpha(base, alpha);
		roundRect(ctx, x, y + 1, w, rowH - 2, 3);
		ctx.fill();

		if (w > 4) {
			const gr = ctx.createLinearGradient(0, y + 1, 0, y + rowH - 2);
			gr.addColorStop(0, 'rgba(255,255,255,0.20)');
			gr.addColorStop(0.45, 'rgba(255,255,255,0.04)');
			gr.addColorStop(1, 'rgba(0,0,0,0.06)');
			ctx.fillStyle = gr;
			roundRect(ctx, x, y + 1, w, rowH - 2, 3);
			ctx.fill();
		}

		if (selected) {
			ctx.strokeStyle = '#1571ff';
			ctx.lineWidth = 1.5;
			roundRect(ctx, x + 0.75, y + 1.75, w - 1.5, rowH - 3.5, 2.5);
			ctx.stroke();
		}

		if (w > 14 && rowH >= 8) {
			const name = NOTE_NAMES[note.pitch % 12];
			const oct = Math.floor(note.pitch / 12) - 1;
			const label = w > (p.editMode ? 34 : 26) ? `${name}${oct}` : name;
			if (!p.editMode && rowH < 10) ctx.font = '8px Inter, system-ui, sans-serif';
			ctx.fillStyle = `rgba(255,255,255,${selected ? 0.9 : p.editMode ? 0.5 : 0.62})`;
			ctx.fillText(label, x + 2, y + rowH / 2);
			if (!p.editMode && rowH < 10) ctx.font = '9px Inter, system-ui, sans-serif';
		}

		if (w > 10 && p.editMode) {
			ctx.fillStyle = `rgba(255,255,255,${selected ? 0.55 : 0.22})`;
			ctx.beginPath();
			ctx.moveTo(x + w - 1, y + rowH - 5);
			ctx.lineTo(x + w - 1, y + rowH - 2);
			ctx.lineTo(x + w - 7, y + rowH - 2);
			ctx.closePath();
			ctx.fill();
		}
	}

	if (p.showSelbox && p.selbox) {
		const { x0, y0, x1, y1 } = p.selbox;
		const rx = Math.min(x0, x1);
		const ry = Math.min(y0, y1);
		const rw = Math.abs(x1 - x0);
		const rh = Math.abs(y1 - y0);
		ctx.fillStyle = 'rgba(21, 113, 255, 0.07)';
		ctx.fillRect(rx, ry, rw, rh);
		ctx.strokeStyle = 'rgba(21, 113, 255, 0.65)';
		ctx.lineWidth = 1;
		ctx.setLineDash([4, 3]);
		ctx.strokeRect(rx + 0.5, ry + 0.5, rw - 1, rh - 1);
		ctx.setLineDash([]);
	}
}
