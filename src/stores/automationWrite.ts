import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { get, writable } from 'svelte/store';
import {
	laneClipFor,
	lanePoints,
	targetKey,
	withRecordedGesture
} from '../frontend/automation/lanes';
import { updateAutomationClip } from '../frontend/commands/dispatcher';
import { projectEventStore } from '../shared/project/eventStore';
import type { AutomationPoint } from '../types/global';
import { notify } from './status';

/** Lanes waiting for a drag in their plugin's window (ADR-037). Session-only UI state. */
export const writeArmedLanes = writable<Set<string>>(new Set());

interface PluginParamGesture {
	effect_id: string;
	native_id: number;
	points: AutomationPoint[];
}

let listening = false;

function writeGesture(gesture: PluginParamGesture): void {
	const armed = get(writeArmedLanes);
	const key = `${gesture.effect_id}/${gesture.native_id}`;
	for (const track of get(projectEventStore).tracks) {
		for (const lane of track.automation_lanes) {
			if (!armed.has(lane.id) || targetKey(lane.target) !== key) continue;
			const points = withRecordedGesture(lanePoints(lane), gesture.points);
			updateAutomationClip(track.id, lane.id, laneClipFor(lane, points)).catch((e) =>
				notify(`Erreur automation : ${(e as Error)?.message ?? e}`, 'error')
			);
		}
	}
}

function startListening(): void {
	if (listening) return;
	listening = true;
	writeArmedLanes.subscribe((lanes) => {
		invoke('set_automation_write_armed', { armed: lanes.size > 0 }).catch(console.error);
	});
	projectEventStore.subscribe((project) => {
		const existing = new Set(
			project.tracks.flatMap((track) => track.automation_lanes.map((lane) => lane.id))
		);
		writeArmedLanes.update((armed) => {
			const kept = new Set([...armed].filter((id) => existing.has(id)));
			return kept.size === armed.size ? armed : kept;
		});
	});
	listen<PluginParamGesture>('plugin-param-gesture', (event) => writeGesture(event.payload)).catch(
		() => {
			listening = false;
		}
	);
}

export function toggleLaneWrite(laneId: string): void {
	startListening();
	writeArmedLanes.update((armed) => {
		const next = new Set(armed);
		if (!next.delete(laneId)) next.add(laneId);
		return next;
	});
}
