<script lang="ts">
	import PianoRollControlLane from '../PianoRollControlLane.svelte';
	import PanelResizeHandle from '../PanelResizeHandle.svelte';
	import {
		ADD_ROW_HEIGHT,
		clampLaneHeight,
		isExpressionLane,
		laneKey,
		lanePoints,
		replaceLanePoints,
		toControlPoints,
		toLanePoints,
		type ControlLane,
		type LanePoint
	} from '../../frontend/pianoRoll/controlLanes';
	import {
		expressionLanePoints,
		noteRange,
		withExpressionPoints,
		type LaneRange
	} from '../../frontend/pianoRoll/noteExpressionLanes';
	import type { MidiControlPoint, MidiNote } from '../../types/global';

	interface LaneRow {
		lane: ControlLane;
		source: LanePoint[];
		range: LaneRange | null;
		editable: boolean;
		commit: (points: LanePoint[]) => Promise<unknown>;
	}

	export let lanes: ControlLane[];
	export let laneHeight: number;
	export let showAddRow: boolean;
	export let scrollLeft: number;
	export let controls: MidiControlPoint[];
	/** The one selected note, whose curves the expression lanes edit; `null` leaves them inert. */
	export let expressionNote: MidiNote | null;
	export let pxPerBeat: number;
	export let widthPx: number;
	export let color: string;
	export let quantize: number;
	export let commit: (next: MidiControlPoint[]) => Promise<unknown>;
	export let commitNote: (next: MidiNote) => Promise<unknown>;

	const NO_POINTS: LanePoint[] = [];
	let heightAtStart = laneHeight;

	function buildRow(
		lane: ControlLane,
		note: MidiNote | null,
		current: MidiControlPoint[]
	): LaneRow {
		if (isExpressionLane(lane)) {
			return {
				lane,
				source: note ? expressionLanePoints(note, lane) : NO_POINTS,
				range: note ? noteRange(note) : null,
				editable: note !== null,
				commit: (points) =>
					note ? commitNote(withExpressionPoints(note, lane, points)) : Promise.resolve()
			};
		}
		return {
			lane,
			source: toLanePoints(lanePoints(current, lane)),
			range: null,
			editable: true,
			commit: (points) => commit(replaceLanePoints(current, lane, toControlPoints(lane, points)))
		};
	}

	$: rows = lanes.map((lane) => buildRow(lane, expressionNote, controls));
</script>

<div class="lanes-wrapper">
	{#if lanes.length > 0}
		<PanelResizeHandle
			axis="vertical"
			side="start"
			on:resizestart={() => (heightAtStart = laneHeight)}
			on:resize={(e) => (laneHeight = clampLaneHeight(heightAtStart + e.detail))}
		/>
	{/if}
	<div class="lanes-track" style="transform: translateX({-scrollLeft}px)">
		{#each rows as row (laneKey(row.lane))}
			<div class="lane-row" style="height: {laneHeight}px">
				<PianoRollControlLane
					lane={row.lane}
					height={laneHeight}
					source={row.source}
					range={row.range}
					editable={row.editable}
					{pxPerBeat}
					{widthPx}
					{color}
					{quantize}
					commit={row.commit}
				/>
			</div>
		{/each}
	</div>
	{#if showAddRow}
		<div class="lane-add-spacer" style="height: {ADD_ROW_HEIGHT}px"></div>
	{/if}
</div>

<style lang="scss">
	@use '../../variables.scss' as *;

	.lanes-wrapper {
		position: relative;
		flex-shrink: 0;
		overflow: hidden;
	}

	.lanes-track {
		will-change: transform;
	}

	.lane-row {
		box-sizing: border-box;
		border-top: 1px solid $border-subtle;
		overflow: hidden;
	}
</style>
