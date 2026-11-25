import { themeVar } from './theme';

/** Palette d'identité par format de plugin (badges du sélecteur, des slots d'effet et de l'instrument). */
const FORMAT_COLORS: Record<string, string> = {
	Internal: '#8b5cf6',
	VST3: '#1571ff',
	CLAP: '#22c55e',
	AU: '#f59e0b'
};

export function pluginFormatColor(format: string): string {
	return FORMAT_COLORS[format] ?? themeVar('text-muted');
}
