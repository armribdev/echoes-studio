import { writable, derived, readable, get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { t } from 'svelte-i18n';
import { isSamplerInstrument, SAMPLER_INSTRUMENT_ID } from '../shared/project/sampler';
import { notify } from './status';

export interface PluginInfo {
	id: string;
	name: string;
	vendor: string;
	format: 'VST3' | 'CLAP' | 'AU' | string;
	categories: string[];
	is_instrument: boolean;
	num_sidechain_inputs: number;
}

/** Built-in insert effects shown alongside scanned plugins in the effect picker. */
export const BUILTIN_EFFECT_PLUGINS: PluginInfo[] = [
	{
		id: 'eq',
		name: 'EQ 3-bandes',
		vendor: 'Echoes Studio',
		format: 'Internal',
		categories: ['EQ'],
		is_instrument: false,
		num_sidechain_inputs: 0
	},
	{
		id: 'compressor',
		name: 'Compresseur',
		vendor: 'Echoes Studio',
		format: 'Internal',
		categories: ['Dynamics'],
		is_instrument: false,
		num_sidechain_inputs: 0
	},
	{
		id: 'reverb',
		name: 'Réverb',
		vendor: 'Echoes Studio',
		format: 'Internal',
		categories: ['Reverb'],
		is_instrument: false,
		num_sidechain_inputs: 0
	},
	{
		id: 'delay',
		name: 'Délai',
		vendor: 'Echoes Studio',
		format: 'Internal',
		categories: ['Delay'],
		is_instrument: false,
		num_sidechain_inputs: 0
	}
];

/** Picker entry for the built-in synth, which a track plays when its `instrument` is null. */
export const BUILTIN_INSTRUMENT_PLUGIN: PluginInfo = {
	id: 'polysynth',
	name: 'PolySynth',
	vendor: 'Echoes Studio',
	format: 'Internal',
	categories: ['Synth'],
	is_instrument: true,
	num_sidechain_inputs: 0
};

/** Picker entry for the built-in drum sampler (ADR-028). */
export const BUILTIN_SAMPLER_PLUGIN: PluginInfo = {
	id: SAMPLER_INSTRUMENT_ID,
	name: 'Sampler',
	vendor: 'Echoes Studio',
	format: 'Internal',
	categories: ['Drums'],
	is_instrument: true,
	num_sidechain_inputs: 0
};

/** The engine's `instrument` id for a picked plugin (`null` = built-in PolySynth). */
export function instrumentIdOf(plugin: PluginInfo): string | null {
	return plugin.id === BUILTIN_INSTRUMENT_PLUGIN.id ? null : plugin.id;
}

// All discovered plugins (filled by the scan that starts with the app and by manual rescans).
export const plugins = writable<PluginInfo[]>([]);

const pendingPluginWork = writable(0);
/** True while a plugin list is loading or a scan runs; several can overlap. */
export const pluginsLoading = derived(pendingPluginWork, (pending) => pending > 0);

function startPluginWork() {
	pendingPluginWork.update((pending) => pending + 1);
}

function endPluginWork() {
	pendingPluginWork.update((pending) => pending - 1);
}

export async function loadPlugins(): Promise<void> {
	startPluginWork();
	try {
		plugins.set(await invoke<PluginInfo[]>('list_plugins'));
	} catch (e) {
		console.error('[plugins] list_plugins failed:', e);
	} finally {
		endPluginWork();
	}
}

/** Lists new or changed plugin files; the engine does not list the others again. */
export async function rescanPlugins(): Promise<void> {
	startPluginWork();
	try {
		plugins.set(await invoke<PluginInfo[]>('scan_plugins'));
	} catch (e) {
		console.error('[plugins] scan_plugins failed:', e);
	} finally {
		endPluginWork();
	}
	await announceNewlyBlockedPlugins();
	await refreshBlockedPlugins();
}

/**
 * The engine scans the plugins while the window is already open. Stays "loading" until its
 * `plugin-scan-finished` event, then reads the list and announces the blocked files.
 */
export function followStartupPluginScan(): () => void {
	let waiting = false;
	let stopped = false;
	let unlisten: UnlistenFn | null = null;

	const onScanned = async () => {
		if (waiting) {
			waiting = false;
			endPluginWork();
		}
		await loadPlugins();
		await announceNewlyBlockedPlugins();
		await refreshBlockedPlugins();
	};

	listen('plugin-scan-finished', onScanned).then(async (fn) => {
		if (stopped) {
			fn();
			return;
		}
		unlisten = fn;
		waiting = true;
		startPluginWork();
		try {
			const running = await invoke<boolean>('plugin_scan_running');
			if (!running && waiting) await onScanned();
		} catch (e) {
			console.error('[plugins] plugin_scan_running failed:', e);
			if (waiting) {
				waiting = false;
				endPluginWork();
			}
		}
	});

	return () => {
		stopped = true;
		unlisten?.();
	};
}

/** A plugin file the scan disabled because it crashed or stopped answering. */
export interface BlockedPlugin {
	path: string;
	name: string;
	reason: 'crashed' | 'timed_out';
}

/** Display copy of the engine's blocklist, refreshed after every scan or re-enable. */
export const blockedPlugins = writable<BlockedPlugin[]>([]);

export async function refreshBlockedPlugins(): Promise<void> {
	try {
		blockedPlugins.set(await invoke<BlockedPlugin[]>('list_blocked_plugins'));
	} catch (e) {
		console.error('[plugins] list_blocked_plugins failed:', e);
	}
}

/** The engine scans the file again at once, so a plugin that still crashes is blocked again. */
export async function reenableBlockedPlugin(path: string): Promise<void> {
	startPluginWork();
	try {
		plugins.set(await invoke<PluginInfo[]>('reenable_blocked_plugin', { path }));
	} catch (e) {
		console.error('[plugins] reenable_blocked_plugin failed:', e);
	} finally {
		endPluginWork();
	}
	await announceNewlyBlockedPlugins();
	await refreshBlockedPlugins();
}

const BLOCKED_ANNOUNCEMENT_MS = 10000;

export function blockedAnnouncement(
	blocked: BlockedPlugin[]
): { key: string; values: Record<string, string | number> } | null {
	if (blocked.length === 0) return null;
	if (blocked.length > 1)
		return { key: 'blockedPlugins.announcedMany', values: { n: blocked.length } };
	const [plugin] = blocked;
	const key =
		plugin.reason === 'crashed'
			? 'blockedPlugins.announcedCrashed'
			: 'blockedPlugins.announcedTimedOut';
	return { key, values: { name: plugin.name } };
}

/** Tells the user, once, about the files blocked since the last call. */
export async function announceNewlyBlockedPlugins(): Promise<void> {
	try {
		const blocked = await invoke<BlockedPlugin[]>('take_newly_blocked_plugins');
		const announcement = blockedAnnouncement(blocked);
		if (!announcement) return;
		notify(
			get(t)(announcement.key, { values: announcement.values }),
			'error',
			BLOCKED_ANNOUNCEMENT_MS
		);
	} catch (e) {
		console.error('[plugins] take_newly_blocked_plugins failed:', e);
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
