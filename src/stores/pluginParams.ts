import { get, writable } from 'svelte/store';
import { fetchPluginParams, type PluginParamInfo } from '../frontend/modulation/targets';

const params = writable<Record<string, PluginParamInfo[]>>({});
const inFlight = new Set<string>();

/** Parameters of loaded plugin placements, keyed by placement (effect id or `instrument:{track}`). */
export const pluginParamsByPlacement = { subscribe: params.subscribe };

export async function reloadPluginParams(placementKey: string): Promise<void> {
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
