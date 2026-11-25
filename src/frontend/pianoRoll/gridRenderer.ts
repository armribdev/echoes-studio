import { get } from 'svelte/store';
import { tempo } from '../../stores/tempo';
import { NOTE_H, TOTAL_PITCHES } from './coords';
import { themeColor } from '../theme';

const BLACK_KEYS = new Set([1, 3, 6, 8, 10]);

export interface GridDrawParams {
	ctx: CanvasRenderingContext2D;
	width: number;
	height: number;
	editMode: boolean;
	zoom: number;
	quantize: number;
	scrollLeft: number;
	viewportWidth: number;
	previewPitchMin: number;
	previewPitchMax: number;
	previewRowH: number;
}

function pitchToY(p: GridDrawParams, pitch: number): number {
	if (p.editMode) return (TOTAL_PITCHES - 1 - pitch) * NOTE_H;
	return (p.previewPitchMax - pitch) * p.previewRowH;
}

function noteRowH(p: GridDrawParams): number {
	return p.editMode ? NOTE_H : p.previewRowH;
}

/** Fond + grille horizontale/verticale (sans notes ni playhead). */
export function drawGridLayer(p: GridDrawParams): void {
	const { ctx, width: W, height: H } = p;
	ctx.clearRect(0, 0, W, H);

	const pStart = p.editMode ? 0 : p.previewPitchMin;
	const pEnd = p.editMode ? TOTAL_PITCHES - 1 : p.previewPitchMax;
	const rowH = noteRowH(p);

	for (let pitch = pStart; pitch <= pEnd; pitch++) {
		const y = pitchToY(p, pitch);
		const isB = BLACK_KEYS.has(pitch % 12);
		ctx.fillStyle = themeColor(isB ? 'background-color' : 'panel-background-color');
		ctx.fillRect(0, y, W, rowH);
	}

	ctx.fillStyle = themeColor('surface-1');
	for (let pitch = pStart; pitch <= pEnd; pitch++) {
		if (pitch % 12 === 0) ctx.fillRect(0, pitchToY(p, pitch), W, rowH);
	}

	ctx.lineWidth = 0.5;
	for (let pitch = pStart; pitch <= pEnd + 1; pitch++) {
		const y = p.editMode ? pitchToY(p, pitch) : (p.previewPitchMax - pitch + 1) * p.previewRowH;
		const pitchForLine = p.editMode ? pitch : Math.min(pitch, p.previewPitchMax);
		ctx.strokeStyle = themeColor(pitchForLine % 12 === 0 ? 'border-strong' : 'border-faint');
		ctx.beginPath();
		ctx.moveTo(0, y);
		ctx.lineTo(W, y);
		ctx.stroke();
	}

	const beatsPerMeasure = get(tempo).numerator || 4;
	const totalBeats = W / p.zoom;
	const viewStart = p.scrollLeft / p.zoom;
	const viewEnd = viewStart + p.viewportWidth / p.zoom;
	const beatStart = Math.max(0, Math.floor(viewStart / p.quantize) * p.quantize);
	const beatEnd = Math.min(totalBeats, Math.ceil(viewEnd / p.quantize) * p.quantize + p.quantize);

	for (let b = beatStart; b <= beatEnd; b += p.quantize) {
		const x = b * p.zoom;
		const isMeasure = Math.abs(b % beatsPerMeasure) < 0.001;
		const isBeat = Math.abs(b % 1) < 0.001;
		ctx.strokeStyle = themeColor(
			isMeasure ? 'border-strong' : isBeat ? 'border-subtle' : 'border-faint'
		);
		ctx.lineWidth = isMeasure ? 1 : 0.5;
		ctx.beginPath();
		ctx.moveTo(x, 0);
		ctx.lineTo(x, H);
		ctx.stroke();
	}
}
