<script lang="ts">
	import PianoRollControlLane from '../PianoRollControlLane.svelte';
	import PanelResizeHandle from '../PanelResizeHandle.svelte';
	import {
		ADD_ROW_HEIGHT,
		clampLaneHeight,
		laneKey,
		type ControlLane
	} from '../../frontend/pianoRoll/controlLanes';
	import type { MidiControlPoint } from '../../types/global';

	export let lanes: ControlLane[];
	export let laneHeight: number;
	export let showAddRow: boolean;
	export let scrollLeft: number;
	export let controls: MidiControlPoint[];
	export let pxPerBeat: number;
	export let widthPx: number;
	export let color: string;
	export let quantize: number;
	export let commit: (next: MidiControlPoint[]) => Promise<unknown>;

	let heightAtStart = laneHeight;
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
		{#each lanes as lane (laneKey(lane))}
			<div class="lane-row" style="height: {laneHeight}px">
				<PianoRollControlLane
					{lane}
					height={laneHeight}
					{controls}
					{pxPerBeat}
					{widthPx}
					{color}
					{quantize}
					{commit}
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
