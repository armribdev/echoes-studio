import { derived, writable } from 'svelte/store';
import { projectEventStore } from '../shared/project/eventStore';
import { targetKey } from '../frontend/automation/lanes';

/** A right-click on a knob or fader that can be automated; `key` is its `ModulationTargetOption` key. */
export interface AutomationRequest {
	key: string;
	x: number;
	y: number;
}

export const automationRequest = writable<AutomationRequest | null>(null);

/** Per track, the targets driven by a lane with points. An empty lane leaves the knob in charge (ADR-025). */
export const automatedTargets = derived(projectEventStore, ($project) => {
	const byTrack: Record<string, Set<string>> = {};
	for (const track of $project.tracks) {
		const driven = track.automation_lanes.filter((lane) => lane.clips.some((clip) => clip.points.length > 0));
		byTrack[track.id] = new Set(driven.map((lane) => targetKey(lane.target)));
	}
	return byTrack;
});
