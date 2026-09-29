import { derived, writable } from 'svelte/store';
import { projectEventStore } from '../shared/project/eventStore';
import { automationRange, automationValueAt, targetKey, toPlainValue } from '../frontend/automation/lanes';
import { transport } from './transport';

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

/**
 * Per track and target key, the plain value the lane asks for at the playhead: what a knob shows
 * while a lane drives it. Track volume is the engine's linear gain (0..2).
 */
export const automatedValues = derived([projectEventStore, transport], ([$project, $transport]) => {
	const byTrack: Record<string, Record<string, number>> = {};
	for (const track of $project.tracks) {
		for (const lane of track.automation_lanes) {
			const normalised = automationValueAt(lane, $transport.position);
			const range = automationRange(lane.target, track.effects, {});
			if (normalised === undefined || !range) continue;
			(byTrack[track.id] ??= {})[targetKey(lane.target)] = toPlainValue(normalised, range);
		}
	}
	return byTrack;
});
