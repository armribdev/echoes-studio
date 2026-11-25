import type { PluginParamInfo } from './targets';

/** A page of parameters a plugin offers first (`clap.remote-controls`). */
export interface PluginRemotePage {
	section: string;
	name: string;
	native_ids: number[];
}

function pageLabel(page: PluginRemotePage): string {
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
