export interface RulerMinorTick {
	x: number;
}

export interface RulerLabeledTick {
	x: number;
	label: string;
}

export interface RulerTicks {
	major: RulerLabeledTick[];
	beat: RulerLabeledTick[];
	minor: RulerMinorTick[];
}

export interface ComputeRulerTicksOptions {
	/** Pixels par beat (zoom piano roll ou px/beat dérivé de l'échelle timeline). */
	pxPerBeat: number;
	numerator: number;
	/** Décalage horizontal du viewport (scrollLeft). */
	scrollPx?: number;
	/** Largeur visible ; omis = timeline entière (arrangement). */
	viewportWidthPx?: number;
	/** Dernier numéro de mesure (1-based). */
	maxBars?: number;
}

/** Calcule les ticks de la règle temporelle (même logique que l'arrangement). */
export function computeRulerTicks(options: ComputeRulerTicksOptions): RulerTicks {
	const { pxPerBeat, numerator, scrollPx = 0, viewportWidthPx, maxBars = 2000 } = options;

	const pxPerBar = numerator * pxPerBeat;
	const viewMin = scrollPx - 40;
	const viewMax = viewportWidthPx != null ? scrollPx + viewportWidthPx + 40 : Infinity;
	const startBeat = scrollPx / pxPerBeat;

	const major: RulerLabeledTick[] = [];
	const beat: RulerLabeledTick[] = [];
	const minor: RulerMinorTick[] = [];

	let barLabelEvery = 1;
	while (barLabelEvery * pxPerBar < 40 && barLabelEvery < 4096) barLabelEvery *= 2;

	const showBeatLabels = barLabelEvery === 1 && pxPerBeat >= 32;

	const inView = (x: number) => viewportWidthPx == null || (x >= viewMin && x <= viewMax);

	for (let bar = 1; bar <= maxBars; bar++) {
		const barBeat = (bar - 1) * numerator;
		const barX = barBeat * pxPerBeat;

		if (viewportWidthPx != null && barX > viewMax) break;
		if (viewportWidthPx != null && barBeat + numerator < startBeat - 1) continue;

		if ((bar - 1) % barLabelEvery === 0) {
			if (inView(barX)) major.push({ x: barX, label: `${bar}` });
		} else if (pxPerBar >= 16 && inView(barX)) {
			minor.push({ x: barX });
		}

		if (pxPerBeat >= 16) {
			for (let beatNum = 2; beatNum <= numerator; beatNum++) {
				const beatX = (barBeat + beatNum - 1) * pxPerBeat;
				if (!inView(beatX)) continue;
				if (showBeatLabels) {
					beat.push({ x: beatX, label: `${bar}.${beatNum}` });
				} else {
					minor.push({ x: beatX });
				}
			}
		}
	}

	return { major, beat, minor };
}

/** maxBars pour ~30 min de timeline (arrangement). */
export function defaultArrangeMaxBars(bpm: number, numerator: number): number {
	const barSec = (numerator * 60) / bpm;
	return Math.min(Math.ceil(1800 / barSec) + 4, 2000);
}
