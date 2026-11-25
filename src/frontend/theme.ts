/** Tokens of `src/variables.scss` that canvases and workers need at runtime (`:root { --es-* }` in app.scss). */
export const THEME_TOKENS = [
	'background-color',
	'panel-background-color',
	'text-color',
	'text-strong',
	'text-secondary',
	'text-muted',
	'text-subtle',
	'surface-1',
	'surface-2',
	'border-faint',
	'border-subtle',
	'border-strong',
	'accent-color',
	'accent-wash',
	'accent-border-strong',
	'color-success',
	'color-warning',
	'on-solid',
	'meter-low',
	'meter-mid',
	'meter-high',
	'clip-audio-wave',
	'note-sheen-top',
	'note-sheen-mid',
	'note-sheen-bottom'
] as const;

export type ThemeToken = (typeof THEME_TOKENS)[number];

const resolved = new Map<ThemeToken, string>();

export function themeColor(token: ThemeToken): string {
	const cached = resolved.get(token);
	if (cached !== undefined) return cached;
	if (typeof document === 'undefined') return '';
	const value = getComputedStyle(document.documentElement).getPropertyValue(`--es-${token}`).trim();
	if (value !== '') resolved.set(token, value);
	return value;
}

/** CSS value to put in an inline `style`, for colors that must follow the token. */
export function themeVar(token: ThemeToken): string {
	return `var(--es-${token})`;
}
