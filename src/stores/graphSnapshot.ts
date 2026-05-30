import { writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { GraphSnapshotDto } from '../frontend/graph/types';
import { ev } from '../shared/protocol/events';

/** Events that change graph topology — trigger snapshot refresh. */
const GRAPH_REFRESH_TYPES = new Set<string>([
	ev.TRACK_CREATED,
	ev.TRACK_DELETED,
	ev.PROJECT_RESET,
	ev.EFFECT_ADDED,
	ev.EFFECT_REMOVED,
	ev.EFFECT_UPDATED,
	ev.MODULATOR_ADDED,
	ev.MODULATOR_REMOVED,
	ev.MODULATION_ROUTE_ADDED,
	ev.MODULATION_ROUTE_REMOVED,
	ev.MIDI_CLIP_ADDED,
	ev.MIDI_CLIP_REMOVED,
]);

function createGraphSnapshotStore() {
	const { subscribe, set, update } = writable<GraphSnapshotDto | null>(null);
	let unlisten: UnlistenFn | null = null;
	let connected = false;

	async function refresh() {
		try {
			const snap = await invoke<GraphSnapshotDto>('get_graph_snapshot');
			set(snap);
		} catch (e) {
			console.error('get_graph_snapshot failed:', e);
		}
	}

	async function connect() {
		if (connected) return;
		connected = true;
		await refresh();
		unlisten = await listen<{ type: string }>('engine-event', ({ payload }) => {
			if (payload?.type && GRAPH_REFRESH_TYPES.has(payload.type)) {
				void refresh();
			}
		});
	}

	function disconnect() {
		unlisten?.();
		unlisten = null;
		connected = false;
	}

	return {
		subscribe,
		connect,
		disconnect,
		refresh,
	};
}

export const graphSnapshot = createGraphSnapshotStore();
