import type { ArpMode, ArpRate, ArpeggiatorSettings } from '../../types/global';

/** What the engine uses for a track that was never configured (ADR-030). */
export const DEFAULT_ARPEGGIATOR: ArpeggiatorSettings = {
	enabled: false,
	mode: 'up',
	rate: 'sixteenth',
	triplet: false,
	gate_percent: 80,
	octaves: 1
};

export const ARP_MODE_OPTIONS: { value: ArpMode; label: string }[] = [
	{ value: 'up', label: 'Haut' },
	{ value: 'down', label: 'Bas' },
	{ value: 'up_down', label: 'Haut-bas' },
	{ value: 'played', label: 'Ordre de jeu' },
	{ value: 'random', label: 'Aléatoire' }
];

export const ARP_RATE_OPTIONS: { value: ArpRate; label: string }[] = [
	{ value: 'quarter', label: '1/4' },
	{ value: 'eighth', label: '1/8' },
	{ value: 'sixteenth', label: '1/16' },
	{ value: 'thirty_second', label: '1/32' }
];
