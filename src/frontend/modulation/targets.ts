import { invoke } from '@tauri-apps/api/core';
import type { ModulationTargetKind, ModulationTargetRef, TrackEffect } from '../../types/global';
import { withRemotePages, type PluginRemotePage } from './remotePages';

export interface PluginParamInfo {
	native_id: number;
	name: string;
	module: string;
	min: number;
	max: number;
	default: number;
	automatable: boolean;
	modulatable: boolean;
	hidden: boolean;
}

export interface ModulationTargetOption {
	key: string;
	group: string;
	module: string;
	label: string;
	kind: ModulationTargetKind;
	effectId?: string;
	param: string;
	/** Engine depth for a ±100 % slider: internal effects take depth in plain units. */
	depthScale: number;
	/** Offered in the picker; unlisted options only name existing routes. */
	listed: boolean;
}

export interface InternalParam {
	param: string;
	label: string;
	min: number;
	max: number;
	/** Shown after the value; `%` displays the value x100. */
	unit: string;
	/** Automated in equal steps per octave, mirrors the Rust `*_SCALES` (ADR-025). */
	log?: true;
	/** Modulated in octaves through the `{param}_octaves` target (ADR-019 amendment). */
	inOctaves?: true;
}

// Mirrors the `*_RANGES` and `*_SCALES` tables of the Rust effect nodes, which clamp modulated and automated values to them.
const INTERNAL_PARAMS: Record<string, InternalParam[]> = {
	eq: [
		{ param: 'low_gain_db', label: 'Gain graves', min: -12, max: 12, unit: 'dB' },
		{
			param: 'low_freq',
			label: 'Fréquence graves',
			min: 20,
			max: 20000,
			unit: 'Hz',
			log: true,
			inOctaves: true
		},
		{ param: 'mid_gain_db', label: 'Gain médiums', min: -12, max: 12, unit: 'dB' },
		{
			param: 'mid_freq',
			label: 'Fréquence médiums',
			min: 20,
			max: 20000,
			unit: 'Hz',
			log: true,
			inOctaves: true
		},
		{ param: 'mid_q', label: 'Largeur médiums (Q)', min: 0.1, max: 10, unit: '' },
		{ param: 'high_gain_db', label: 'Gain aigus', min: -12, max: 12, unit: 'dB' },
		{
			param: 'high_freq',
			label: 'Fréquence aigus',
			min: 20,
			max: 20000,
			unit: 'Hz',
			log: true,
			inOctaves: true
		}
	],
	compressor: [
		{ param: 'threshold_db', label: 'Seuil', min: -60, max: 0, unit: 'dB' },
		{ param: 'ratio', label: 'Ratio', min: 1, max: 20, unit: ':1' },
		{ param: 'attack_ms', label: 'Attaque', min: 0.1, max: 200, unit: 'ms', log: true },
		{ param: 'release_ms', label: 'Relâchement', min: 10, max: 2000, unit: 'ms', log: true },
		{ param: 'makeup_db', label: 'Gain de sortie', min: -6, max: 24, unit: 'dB' },
		{ param: 'knee_db', label: 'Genou', min: 0, max: 24, unit: 'dB' }
	],
	reverb: [
		{ param: 'room_size', label: 'Taille', min: 0, max: 1, unit: '%' },
		{ param: 'damping', label: 'Amortissement', min: 0, max: 1, unit: '%' },
		{ param: 'wet', label: 'Mix', min: 0, max: 1, unit: '%' },
		{ param: 'width', label: 'Largeur stéréo', min: 0, max: 1, unit: '%' }
	],
	delay: [
		{ param: 'delay_ms', label: 'Temps', min: 10, max: 2000, unit: 'ms' },
		{ param: 'feedback', label: 'Répétitions', min: 0, max: 0.95, unit: '%' },
		{ param: 'wet', label: 'Mix', min: 0, max: 1, unit: '%' }
	]
};

const INTERNAL_EFFECT_LABELS: Record<string, string> = {
	eq: 'EQ',
	compressor: 'Compresseur',
	reverb: 'Réverb',
	delay: 'Délai'
};

export interface PluginPlacement {
	key: string;
	name: string;
}

export function instrumentPlacementKey(trackId: string): string {
	return `instrument:${trackId}`;
}

function pluginEffectName(effect: TrackEffect): string | null {
	const params = effect.params as { kind: string; params?: { name?: string } };
	return params.kind === 'plugin' ? (params.params?.name ?? 'Plugin') : null;
}

export function pluginPlacements(
	trackId: string,
	effects: TrackEffect[],
	instrumentName: string | null
): PluginPlacement[] {
	const placements = effects.flatMap((effect) => {
		const name = pluginEffectName(effect);
		return name ? [{ key: effect.id, name }] : [];
	});
	if (instrumentName)
		placements.push({ key: instrumentPlacementKey(trackId), name: instrumentName });
	return placements;
}

/** The pages a plugin offers first; `null` when it cannot be asked right now (retry). */
export async function fetchPluginRemotePages(
	placementKey: string
): Promise<PluginRemotePage[] | null> {
	try {
		return await invoke<PluginRemotePage[] | null>('get_plugin_remote_pages', {
			instanceKey: placementKey
		});
	} catch {
		return null;
	}
}

/**
 * Parameters of a loaded plugin placement; empty when the plugin is not loaded.
 * The pages the plugin offers first (`clap.remote-controls`) lead the list.
 */
export async function fetchPluginParams(placementKey: string): Promise<PluginParamInfo[]> {
	try {
		const [params, pages] = await Promise.all([
			invoke<PluginParamInfo[]>('get_plugin_params', { instanceKey: placementKey }),
			fetchPluginRemotePages(placementKey)
		]);
		return pages ? withRemotePages(params, pages) : params;
	} catch {
		return [];
	}
}

/** Display names of the track's effects, numbered when a kind appears twice. */
export function effectGroupNames(effects: TrackEffect[]): Map<string, string> {
	const names = new Map<string, string>();
	const seen = new Map<string, number>();
	for (const effect of effects) {
		const base =
			pluginEffectName(effect) ?? INTERNAL_EFFECT_LABELS[effect.params.kind] ?? effect.params.kind;
		const count = (seen.get(base) ?? 0) + 1;
		seen.set(base, count);
		names.set(effect.id, count > 1 ? `${base} ${count}` : base);
	}
	return names;
}

function internalOptions(
	effectId: string,
	group: string,
	p: InternalParam
): ModulationTargetOption[] {
	const option: ModulationTargetOption = {
		key: `${effectId}/${p.param}`,
		group,
		module: '',
		label: p.label,
		kind: 'effect_param',
		effectId,
		param: p.param,
		depthScale: p.max - p.min,
		listed: true
	};
	if (!p.inOctaves) return [option];
	// Routes created before 2026-09-24 keep their Hz depth: still named, no longer offered.
	return [
		{
			...option,
			key: `${option.key}_octaves`,
			param: `${p.param}_octaves`,
			depthScale: Math.log2(p.max / p.min)
		},
		{ ...option, label: `${p.label} (Hz)`, listed: false }
	];
}

export function buildTargetOptions(
	effects: TrackEffect[],
	placements: PluginPlacement[],
	paramsByPlacement: Record<string, PluginParamInfo[]>
): ModulationTargetOption[] {
	const options: ModulationTargetOption[] = [
		{
			key: 'track_gain',
			group: 'Piste',
			module: '',
			label: 'Volume',
			kind: 'track_gain',
			param: 'gain',
			depthScale: 1,
			listed: true
		}
	];
	const groupNames = effectGroupNames(effects);

	for (const effect of effects) {
		for (const p of INTERNAL_PARAMS[effect.params.kind] ?? []) {
			options.push(...internalOptions(effect.id, groupNames.get(effect.id) ?? '', p));
		}
	}

	// Automatable-only params are modulated by value in the engine (ADR-021 amendment).
	options.push(
		...pluginParamOptions(
			effects,
			placements,
			paramsByPlacement,
			(p) => p.modulatable || p.automatable
		)
	);
	return options;
}

export function internalParam(
	effect: TrackEffect | undefined,
	param: string
): InternalParam | undefined {
	return effect ? INTERNAL_PARAMS[effect.params.kind]?.find((p) => p.param === param) : undefined;
}

/** Track volume, internal effect parameters and automatable plugin parameters (absolute values, ADR-021 / ADR-025). */
export function buildAutomationOptions(
	effects: TrackEffect[],
	placements: PluginPlacement[],
	paramsByPlacement: Record<string, PluginParamInfo[]>
): ModulationTargetOption[] {
	const groupNames = effectGroupNames(effects);
	const options: ModulationTargetOption[] = [
		{
			key: 'track_gain',
			group: 'Piste',
			module: '',
			label: 'Volume',
			kind: 'track_gain',
			param: 'gain',
			depthScale: 1,
			listed: true
		}
	];
	for (const effect of effects) {
		for (const p of INTERNAL_PARAMS[effect.params.kind] ?? []) {
			options.push({
				key: `${effect.id}/${p.param}`,
				group: groupNames.get(effect.id) ?? '',
				module: '',
				label: p.label,
				kind: 'effect_param',
				effectId: effect.id,
				param: p.param,
				depthScale: p.max - p.min,
				listed: true
			});
		}
	}
	options.push(...pluginParamOptions(effects, placements, paramsByPlacement, (p) => p.automatable));
	return options;
}

function pluginParamOptions(
	effects: TrackEffect[],
	placements: PluginPlacement[],
	paramsByPlacement: Record<string, PluginParamInfo[]>,
	accepts: (param: PluginParamInfo) => boolean
): ModulationTargetOption[] {
	const groupNames = effectGroupNames(effects);
	return placements.flatMap((placement) => {
		const group = groupNames.get(placement.key) ?? placement.name;
		return (paramsByPlacement[placement.key] ?? [])
			.filter((p) => accepts(p) && !p.hidden)
			.map((p) => ({
				key: `${placement.key}/${p.native_id}`,
				group,
				module: p.module,
				label: p.name,
				kind: 'plugin_param' as const,
				effectId: placement.key,
				param: String(p.native_id),
				depthScale: 1,
				listed: true
			}));
	});
}

export function findTargetOption(
	options: ModulationTargetOption[],
	target: ModulationTargetRef
): ModulationTargetOption | undefined {
	switch (target.kind) {
		case 'track_gain':
			return options.find((o) => o.kind === 'track_gain');
		case 'effect_param':
			return options.find((o) => o.effectId === target.effect_id && o.param === target.param);
		case 'plugin_param':
			return options.find(
				(o) => o.effectId === target.effect_id && o.param === String(target.native_id)
			);
	}
}

export function matchesSearch(option: ModulationTargetOption, query: string): boolean {
	const needle = query.trim().toLowerCase();
	if (!needle) return true;
	return `${option.group} ${option.module} ${option.label}`.toLowerCase().includes(needle);
}
