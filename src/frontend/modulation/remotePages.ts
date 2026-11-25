import type { PluginParamInfo } from './targets';

/** A page of parameters a plugin offers first (`clap.remote-controls`). */
export interface PluginRemotePage {
	section: string;
	name: string;
	native_ids: number[];
}

export function pageLabel(page: PluginRemotePage): string {
	if (!page.section || page.name.startsWith(page.section)) return page.name;
	return `${page.section} / ${page.name}`;
}

/**
 * The parameters on a page come first, under the page's name, in the plugin's
 * order; the others follow unchanged. A parameter on several pages stays on the first.
 */
export function withRemotePages(
	params: PluginParamInfo[],
	pages: PluginRemotePage[]
): PluginParamInfo[] {
	const byId = new Map(params.map((param) => [param.native_id, param]));
	const placed = new Set<number>();
	const featured: PluginParamInfo[] = [];
	for (const page of pages) {
		const module = pageLabel(page);
		for (const id of page.native_ids) {
			const param = byId.get(id);
			if (!param || placed.has(id)) continue;
			placed.add(id);
			featured.push({ ...param, module });
		}
	}
	if (featured.length === 0) return params;
	return [...featured, ...params.filter((param) => !placed.has(param.native_id))];
}

/** The parameters of a page in the plugin's order; one the plugin does not list is left out. */
export function pageParams(page: PluginRemotePage, params: PluginParamInfo[]): PluginParamInfo[] {
	const byId = new Map(params.map((param) => [param.native_id, param]));
	return page.native_ids.flatMap((id) => {
		const param = byId.get(id);
		return param ? [param] : [];
	});
}

/** `0..1` position of a plain value in the parameter's range, as the engine's queue takes it. */
export function normalisedValue(param: PluginParamInfo, plain: number): number {
	const span = param.max - param.min;
	if (span <= 0) return 0;
	return Math.min(1, Math.max(0, (plain - param.min) / span));
}
