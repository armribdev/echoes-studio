import { invoke } from '@tauri-apps/api/core';

/** A name a plugin gives to notes; `-1` in `port`, `key` or `channel` means every one. */
export interface PluginNoteName {
	name: string;
	port: number;
	key: number;
	channel: number;
}

/** The name of each key a plugin names, by pitch. */
export type KeyNames = ReadonlyMap<number, string>;

/** Names of single keys; a name for every key labels no row. The first name of a key wins. */
export function keyNamesOf(names: PluginNoteName[]): KeyNames {
	const byKey = new Map<number, string>();
	for (const { key, name } of names) {
		if (key >= 0 && name !== '' && !byKey.has(key)) byKey.set(key, name);
	}
	return byKey;
}

export function sameKeyNames(a: KeyNames, b: KeyNames): boolean {
	if (a.size !== b.size) return false;
	for (const [key, name] of a) if (b.get(key) !== name) return false;
	return true;
}

/** The names the plugin of a placement gives to notes; none when it has none or is unknown. */
export async function fetchPluginNoteNames(placementKey: string): Promise<PluginNoteName[]> {
	try {
		const names = await invoke<PluginNoteName[] | null>('get_plugin_note_names', {
			instanceKey: placementKey
		});
		return names ?? [];
	} catch {
		return [];
	}
}
