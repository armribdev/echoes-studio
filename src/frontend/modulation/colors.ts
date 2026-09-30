// Distinct, dark-theme-friendly colors assigned to modulators by index, so a
// source (LFO) and everything it drives (routes, target knob rings) read as
// the same visual object across separate panel sections.
const MODULATOR_COLORS = [
	'#1571ff', // blue
	'#22c55e', // green
	'#f59e0b', // amber
	'#ec4899', // pink
	'#a855f7', // purple
	'#14b8a6', // teal
	'#eab308', // yellow
	'#fb7185' // rose
];

export function modulatorColor(index: number): string {
	return MODULATOR_COLORS[index % MODULATOR_COLORS.length];
}
