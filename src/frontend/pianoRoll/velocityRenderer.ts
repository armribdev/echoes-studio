import { hexAlpha, lighten } from './colorUtils';
import type { MidiNote } from '../../types/global';
import { themeColor } from '../theme';

export const VEL_H = 72;
const VEL_BAR_MIN = 3;

export interface VelocityDrawParams {
	ctx: CanvasRenderingContext2D;
	width: number;
	notes: MidiNote[];
	selectedNoteIds: Set<string>;
	zoom: number;
	quantize: number;
	trackColor: string;
}

export function drawVelocityLayer(p: VelocityDrawParams): void {
	const { ctx, width: W, notes, selectedNoteIds, zoom, quantize, trackColor } = p;
	const H = VEL_H;
	ctx.clearRect(0, 0, W, H);

	// Fond
	ctx.fillStyle = themeColor('background-color');
	ctx.fillRect(0, 0, W, H);

	// Lignes de repère horizontales
	ctx.lineWidth = 0.5;
	[0.25, 0.5, 0.75].forEach((p) => {
		ctx.strokeStyle = themeColor('border-faint');
		const y = H - p * (H - 4);
		ctx.beginPath();
		ctx.moveTo(0, y);
		ctx.lineTo(W, y);
		ctx.stroke();
	});

	// Barres de velocity
	for (const note of notes) {
		const x = note.start * zoom + 1;
		const barH = Math.max(VEL_BAR_MIN, (note.velocity / 127) * (H - 6));
		const barW = Math.max(3, quantize * zoom - 3);
		const y = H - barH;
		const selected = selectedNoteIds.has(note.id);
		const base = selected ? lighten(trackColor, 0.2) : trackColor;
		const alpha = selected ? 0.9 : 0.62;

		// Gradient du bas vers le haut
		const gr = ctx.createLinearGradient(0, y, 0, H);
		gr.addColorStop(0, hexAlpha(base, alpha));
		gr.addColorStop(1, hexAlpha(base, alpha * 0.55));
		ctx.fillStyle = gr;

		// Arrondi en haut uniquement
		const r = Math.min(2, barW / 2);
		ctx.beginPath();
		ctx.moveTo(x + r, y);
		ctx.lineTo(x + barW - r, y);
		ctx.quadraticCurveTo(x + barW, y, x + barW, y + r);
		ctx.lineTo(x + barW, H);
		ctx.lineTo(x, H);
		ctx.lineTo(x, y + r);
		ctx.quadraticCurveTo(x, y, x + r, y);
		ctx.closePath();
		ctx.fill();
	}

	// Séparateur haut
	ctx.strokeStyle = themeColor('border-subtle');
	ctx.lineWidth = 1;
	ctx.beginPath();
	ctx.moveTo(0, 0.5);
	ctx.lineTo(W, 0.5);
	ctx.stroke();
}
