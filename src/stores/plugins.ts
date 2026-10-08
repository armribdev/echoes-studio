import { writable, derived, readable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { isSamplerInstrument, SAMPLER_INSTRUMENT_ID } from '../shared/project/sampler';

export interface PluginInfo {
	id: string;
	name: string;
	vendor: string;
	format: 'VST3' | 'CLAP' | 'AU' | string;
	categories: string[];
	is_instrument: boolean;
}

/** Built-in insert effects shown alongside scanned plugins in the effect picker. */
export const BUILTIN_EFFECT_PLUGINS: PluginInfo[] = [
	{
		id: 'eq',
		name: 'EQ 3-bandes',
		vendor: 'Echoes Studio',
		format: 'Internal',
		categories: ['EQ'],
		is_instrument: false
	},
	{
		id: 'compressor',
		name: 'Compresseur',
		vendor: 'Echoes Studio',
		format: 'Internal',
		categories: ['Dynamics'],
		is_instrument: false
	},
	{
		id: 'reverb',
		name: 'Réverb',
		vendor: 'Echoes Studio',
		format: 'Internal',
		categories: ['Reverb'],
		is_instrument: false
	},
	{
		id: 'delay',
		name: 'Délai',
		vendor: 'Echoes Studio',
		format: 'Internal',
		categories: ['Delay'],
		is_instrument: false
	}
];

/** Picker entry for the built-in synth, which a track plays when its `instrument` is null. */
export const BUILTIN_INSTRUMENT_PLUGIN: PluginInfo = {
	id: 'polysynth',
	name: 'PolySynth',
	vendor: 'Echoes Studio',
	format: 'Internal',
	categories: ['Synth'],
	is_instrument: true
};

/** Picker entry for the built-in drum sampler (ADR-028). */
export const BUILTIN_SAMPLER_PLUGIN: PluginInfo = {
	id: SAMPLER_INSTRUMENT_ID,
	name: 'Sampler',
	vendor: 'Echoes Studio',
	format: 'Internal',
	categories: ['Drums'],
	is_instrument: true
};

/** The engine's `instrument` id for a picked plugin (`null` = built-in PolySynth). */
export function instrumentIdOf(plugin: PluginInfo): string | null {
	return plugin.id === BUILTIN_INSTRUMENT_PLUGIN.id ? null : plugin.id;
}

// All discovered plugins (populated at app init + on manual rescan).
export const plugins = writable<PluginInfo[]>([]);
export const pluginsLoading = writable(false);

export async function loadPlugins(): Promise<void> {
	pluginsLoading.set(true);
	try {
		const list = await invoke<PluginInfo[]>('list_plugins');
		plugins.set(list);
	} catch (e) {
		console.error('[plugins] list_plugins failed:', e);
	} finally {
		pluginsLoading.set(false);
	}
}

export async function rescanPlugins(): Promise<void> {
	pluginsLoading.set(true);
	try {
		const list = await invoke<PluginInfo[]>('scan_plugins');
		plugins.set(list);
	} catch (e) {
		console.error('[plugins] scan_plugins failed:', e);
	} finally {
		pluginsLoading.set(false);
	}
}

/** Filtered + sorted subset for the plugin picker. */
export function filteredPlugins(query: string) {
	return derived(plugins, ($p) => {
		const q = query.toLowerCase();
		return $p
			.filter(
				(p) =>
					!q ||
					p.name.toLowerCase().includes(q) ||
					p.vendor.toLowerCase().includes(q) ||
					p.categories.some((c) => c.toLowerCase().includes(q))
			)
			.sort((a, b) => a.name.localeCompare(b.name));
	});
}

export type UnavailableReason = 'not_installed' | 'unsupported_format' | 'load_failed' | 'crashed';

export interface UnavailablePlugin {
	/** Effect id, or `instrument:{trackId}` for a track's instrument. */
	instance_key: string;
	plugin_id: string;
	reason: UnavailableReason;
}

export const UNAVAILABLE_REASON_LABEL: Record<UnavailableReason, string> = {
	not_installed: 'Introuvable',
	unsupported_format: 'Format non pris en charge',
	load_failed: 'Échec du chargement',
	crashed: 'Planté'
};

/** Plugins the engine could not load, keyed by instance key. */
export const unavailablePlugins = readable<Map<string, UnavailablePlugin>>(new Map(), (set) => {
	const apply = (list: UnavailablePlugin[]) => set(new Map(list.map((p) => [p.instance_key, p])));
	let unlisten: UnlistenFn | null = null;
	let stopped = false;
	let eventReceived = false;
	listen<UnavailablePlugin[]>('plugin-availability', ({ payload }) => {
		eventReceived = true;
		apply(payload);
	}).then((fn) => {
		if (stopped) fn();
		else unlisten = fn;
	});
	invoke<UnavailablePlugin[]>('get_unavailable_plugins')
		.then((list) => {
			// A newer pushed list wins over this initial query.
			if (!eventReceived) apply(list);
		})
		.catch((e) => console.error('[plugins] get_unavailable_plugins failed:', e));
	return () => {
		stopped = true;
		unlisten?.();
	};
});

/** Readable fallback name when the plugin is not in the scanned list, e.g. `clap:com.u-he.Diva` → `Diva`. */
export function pluginDisplayName(pluginId: string): string {
	const bare = pluginId.replace(/^[a-z0-9]+:/i, '');
	return bare.split('.').pop() || bare;
}

/** Name of a track's plugin instrument, or null for a built-in instrument. */
export function instrumentDisplayName(
	list: PluginInfo[],
	instrument: string | null | undefined
): string | null {
	if (!instrument || isSamplerInstrument(instrument)) return null;
	return list.find((p) => p.id === instrument)?.name ?? pluginDisplayName(instrument);
}
