import { updateModulator } from '../commands/dispatcher';
import { notify } from '../../stores/status';
import type { ModulatorKind, ProjectModulator } from '../../types/global';

export interface ModulatorKindInfo {
	kind: ModulatorKind;
	title: string;
	/** Short prefix used to number modulators of this kind ("LFO 1", "Env 2"). */
	prefix: string;
	description: string;
	/** SVG path drawn in a 14 × 8 box. */
	icon: string;
}

export const MODULATOR_KINDS: ModulatorKindInfo[] = [
	{
		kind: 'lfo',
		title: 'LFO',
		prefix: 'LFO',
		description: 'Oscillation régulière, libre ou calée sur le tempo',
		icon: 'M0 4 C2.5 -1.5 4.5 -1.5 7 4 C9.5 9.5 11.5 9.5 14 4'
	},
	{
		kind: 'envelope',
		title: 'Enveloppe',
		prefix: 'Env',
		description: 'Se déclenche avec les notes jouées (ADSR)',
		icon: 'M0 7.5 L2.5 0.5 L5.5 3.5 L10 3.5 L14 7.5'
	},
	{
		kind: 'audio_follower',
		title: 'Suiveur audio',
		prefix: 'Suiveur',
		description: 'Suit le volume du son de la piste',
		icon: 'M0 7 L2 5 L3.5 6 L5 2 L7 4 L8.5 1 L10.5 5 L12 4 L14 6'
	},
	{
		kind: 'macro',
		title: 'Macro',
		prefix: 'Macro',
		description: 'Un seul bouton qui pilote plusieurs réglages',
		icon: 'M7 0.5 A3.5 3.5 0 1 1 6.99 0.5 M7 4 L9.5 1.8'
	}
];

export function kindInfo(kind: ModulatorKind): ModulatorKindInfo {
	return MODULATOR_KINDS.find((k) => k.kind === kind) ?? MODULATOR_KINDS[0];
}

/** "LFO 1", "Env 2"… numbered within the modulator's own kind. */
export function modulatorLabel(modulators: ProjectModulator[], mod: ProjectModulator): string {
	const sameKind = modulators.filter((m) => m.kind === mod.kind);
	return `${kindInfo(mod.kind).prefix} ${sameKind.findIndex((m) => m.id === mod.id) + 1}`;
}

export function formatMs(ms: number): string {
	if (ms >= 1000) return `${(ms / 1000).toFixed(2)} s`;
	return `${ms < 10 ? ms.toFixed(1) : ms.toFixed(0)} ms`;
}

export function formatPercent(fraction: number): string {
	return `${Math.round(fraction * 100)}%`;
}

export async function updateModulatorOrNotify(
	trackId: string,
	modulatorId: string,
	changes: Parameters<typeof updateModulator>[2]
): Promise<void> {
	try {
		await updateModulator(trackId, modulatorId, changes);
	} catch (e) {
		notify(`Erreur modulateur : ${(e as Error)?.message ?? e}`, 'error');
	}
}
