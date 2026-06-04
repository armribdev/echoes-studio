import { writable, derived } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';

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
	{ id: 'eq', name: 'EQ 3-bandes', vendor: 'Echoes Studio', format: 'Internal', categories: ['EQ'], is_instrument: false },
	{ id: 'compressor', name: 'Compresseur', vendor: 'Echoes Studio', format: 'Internal', categories: ['Dynamics'], is_instrument: false },
	{ id: 'reverb', name: 'Réverb', vendor: 'Echoes Studio', format: 'Internal', categories: ['Reverb'], is_instrument: false },
	{ id: 'delay', name: 'Délai', vendor: 'Echoes Studio', format: 'Internal', categories: ['Delay'], is_instrument: false },
];

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
