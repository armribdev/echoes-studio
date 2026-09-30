/** Palette partagée pour les pistes (arrange, mixer, piano roll, graph). */
export const TRACK_COLORS = [
	'#ef4444', // red
	'#f97316', // orange
	'#f59e0b', // amber
	'#eab308', // yellow
	'#84cc16', // lime
	'#22c55e', // green
	'#14b8a6', // teal
	'#06b6d4', // cyan
	'#0ea5e9', // sky
	'#3b82f6', // blue
	'#6366f1', // indigo
	'#8b5cf6', // violet
	'#a855f7', // purple
	'#d946ef', // fuchsia
	'#ec4899', // pink
	'#f43f5e', // rose
	'#fb7185', // soft rose
	'#fdba74', // soft orange
	'#86efac', // soft green
	'#67e8f9', // soft cyan
	'#93c5fd', // soft blue
	'#c4b5fd', // soft violet
	'#e2e8f0', // light gray
	'#94a3b8' // slate
] as const;

/** Couleur affichée : override projet si présent, sinon défaut stable depuis l'id. */
export function trackColor(trackId: string, override?: string | null): string {
	if (override && isTrackColor(override)) return override;
	return TRACK_COLORS[stableIndex(trackId) % TRACK_COLORS.length];
}

export function isTrackColor(value: string): boolean {
	return /^#[0-9a-fA-F]{6}$/.test(value);
}

function stableIndex(id: string): number {
	let hash = 2166136261;
	for (let i = 0; i < id.length; i++) {
		hash ^= id.charCodeAt(i);
		hash = Math.imul(hash, 16777619);
	}
	return hash >>> 0;
}
