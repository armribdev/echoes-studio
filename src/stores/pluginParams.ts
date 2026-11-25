import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { get, writable } from 'svelte/store';
import { fetchPluginParams, type PluginParamInfo } from '../frontend/modulation/targets';

const params = writable<Record<string, PluginParamInfo[]>>({});
const inFlight = new Set<string>();
const texts = new Map<string, string | null>();
const MAX_CACHED_TEXTS = 500;
let watchingRescans = false;

/** Parameters of loaded plugin placements, keyed by placement (effect id or `instrument:{track}`). */
export const pluginParamsByPlacement = { subscribe: params.subscribe };

export async function reloadPluginParams(placementKey: string): Promise<void> {
	watchRescans();
	inFlight.add(placementKey);
	const list = await fetchPluginParams(placementKey);
	inFlight.delete(placementKey);
	params.update((all) => ({ ...all, [placementKey]: list }));
}

/** Fetches once; a placement whose plugin was not loaded yet is retried on the next call. */
export function ensurePluginParams(placementKey: string): void {
	if (inFlight.has(placementKey) || get(params)[placementKey]?.length) return;
	reloadPluginParams(placementKey).catch(console.error);
}

function textKey(placementKey: string, nativeId: number, plain: number): string {
	return `${placementKey}/${nativeId}/${plain}`;
}

/** The text already fetched for this value; `undefined` when it was never asked. */
export function cachedPluginParamText(
	placementKey: string,
	nativeId: number,
	plain: number
): string | null | undefined {
	return texts.get(textKey(placementKey, nativeId, plain));
}

/** The plugin's own text for a plain value (`-6.0 dB`); `null` when it has none. */
export async function pluginParamText(
	placementKey: string,
	nativeId: number,
	plain: number
): Promise<string | null> {
	const cached = cachedPluginParamText(placementKey, nativeId, plain);
	if (cached !== undefined) return cached;
	let text: string | null = null;
	try {
		text = await invoke<string | null>('get_plugin_param_text', {
			instanceKey: placementKey,
			nativeId,
			value: plain
		});
	} catch {
		text = null;
	}
	if (texts.size >= MAX_CACHED_TEXTS) texts.clear();
	texts.set(textKey(placementKey, nativeId, plain), text);
	return text;
}

/** Current values in the units the parameter list reports; `null` for one that cannot be read now. */
export async function fetchPluginParamValues(
	placementKey: string,
	nativeIds: number[]
): Promise<(number | null)[]> {
	try {
		return await invoke<(number | null)[]>('get_plugin_param_values', {
			instanceKey: placementKey,
			nativeIds
		});
	} catch {
		return nativeIds.map(() => null);
	}
}

/** Sets a parameter like the plugin's own window; `normalised` is `0..1`. */
export async function setPluginParam(
	placementKey: string,
	nativeId: number,
	normalised: number
): Promise<void> {
	try {
		await invoke('set_plugin_param', { instanceKey: placementKey, nativeId, normalised });
	} catch (error) {
		console.error(error);
	}
}

/** A plugin changed its parameter list or the text of its values: ask again. */
function watchRescans(): void {
	if (watchingRescans) return;
	watchingRescans = true;
	listen('plugin-params-changed', () => {
		texts.clear();
		Object.keys(get(params)).forEach((key) => reloadPluginParams(key).catch(console.error));
	}).catch(() => {
		watchingRescans = false;
	});
}
