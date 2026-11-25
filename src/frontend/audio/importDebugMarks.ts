/**
 * Markers for Chrome Performance (flash debug).
 * `console.timeStamp` shows as vertical marks in the Performance timeline;
 * `performance.mark` alone is easy to miss in the Timings track.
 */
const ENABLED = import.meta.env.DEV;

export function importMark(name: string): void {
	if (!ENABLED) return;
	const label = `echoes:import:${name}`;
	try {
		console.timeStamp(label);
	} catch {
		/* ignore */
	}
	try {
		performance.mark(label);
	} catch {
		/* ignore */
	}
	// Visible in Console even if Timings stays empty.
	console.log(`⏱ ${label}`);
}
