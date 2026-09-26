export interface ArrangeGridDrawParams {
	ctx: CanvasRenderingContext2D;
	/** Largeur/hauteur du buffer canvas (pixels device). */
	width: number;
	height: number;
	/** Pixels CSS par beat. */
	pxPerBeat: number;
	/** devicePixelRatio — alignement des traits sur la grille device. */
	dpr: number;
	numerator: number;
	totalBeats: number;
	gridBeats: number;
}

const MIN_SUBDIV_PX = 8;

function visibleSubdivision(pxPerBeat: number, gridBeats: number): number {
	const candidates = [gridBeats, 0.5, 1, 2, 4];
	for (const step of candidates) {
		if (step * pxPerBeat >= MIN_SUBDIV_PX) return step;
	}
	let multiple = 1;
	while (gridBeats * multiple * pxPerBeat < MIN_SUBDIV_PX) multiple *= 2;
	return gridBeats * multiple;
}

/** Lignes verticales temps/mesures pour la zone pistes (sans interaction). */
export function drawArrangeGrid(p: ArrangeGridDrawParams): void {
	const { ctx, width: W, height: H, pxPerBeat, dpr, numerator, totalBeats, gridBeats } = p;
	ctx.clearRect(0, 0, W, H);

	const beatsPerMeasure = numerator || 4;
	const subdiv = visibleSubdivision(pxPerBeat, gridBeats);
	const showBeats = pxPerBeat >= 6;
	const pxPerBeatDev = pxPerBeat * dpr;
	// ~1 CSS px, centré sur la grille device (pair → entier, impair → .5).
	const lineWidth = Math.max(1, Math.round(dpr));
	const xAlign = lineWidth % 2 === 0 ? 0 : 0.5;

	for (let b = 0; b <= totalBeats + subdiv; b += subdiv) {
		const x = Math.round(b * pxPerBeatDev) + xAlign;
		if (x > W) break;

		const isMeasure = Math.abs(b % beatsPerMeasure) < 1e-6;
		const isBeat = Math.abs(b % 1) < 1e-6;

		if (isMeasure) {
			ctx.strokeStyle = 'rgba(247, 247, 249, 0.14)';
		} else if (isBeat && showBeats) {
			ctx.strokeStyle = 'rgba(247, 247, 249, 0.08)';
		} else if (!isBeat) {
			ctx.strokeStyle = 'rgba(247, 247, 249, 0.04)';
		} else {
			continue;
		}

		ctx.lineWidth = lineWidth;
		ctx.beginPath();
		ctx.moveTo(x, 0);
		ctx.lineTo(x, H);
		ctx.stroke();
	}
}
