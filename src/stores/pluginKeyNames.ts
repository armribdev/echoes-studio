import { listen } from '@tauri-apps/api/event';
import { writable } from 'svelte/store';
import {
	fetchPluginNoteNames,
	keyNamesOf,
	sameKeyNames,
	type KeyNames
} from '../frontend/pianoRoll/pluginKeyNames';

const REFRESH_DELAY_MS = 300;
const NO_NAMES: KeyNames = new Map();

const names = writable<KeyNames>(NO_NAMES);
let shown: KeyNames = NO_NAMES;
let watched: string | null = null;
let latestRequest = 0;
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
let listening = false;

/** The names the plugin of the watched placement gives to notes, by pitch. */
export const pluginKeyNames = { subscribe: names.subscribe };

/** Follows the plugin of a placement (`instrument:{track}`); `null` stops following. */
export function watchPluginKeyNames(placementKey: string | null): void {
	if (placementKey === watched) return;
	watched = placementKey;
	publish(NO_NAMES);
	if (!placementKey) return;
	listenForChanges();
	refresh().catch(console.error);
}

function publish(next: KeyNames): void {
	if (sameKeyNames(shown, next)) return;
	shown = next;
	names.set(next);
}

async function refresh(): Promise<void> {
	const placementKey = watched;
	if (!placementKey) return;
	const request = ++latestRequest;
	const next = keyNamesOf(await fetchPluginNoteNames(placementKey));
	if (request === latestRequest && placementKey === watched) publish(next);
}

/** A VST3 plugin does not say when its names change with the program: read again after an edit. */
function listenForChanges(): void {
	if (listening) return;
	listening = true;
	const scheduleRefresh = () => {
		clearTimeout(refreshTimer);
		refreshTimer = setTimeout(() => refresh().catch(console.error), REFRESH_DELAY_MS);
	};
	Promise.all([
		listen('plugin-note-names-changed', scheduleRefresh),
		listen('plugin-state-edited', scheduleRefresh)
	]).catch(() => {
		listening = false;
	});
}
