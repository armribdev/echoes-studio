<script lang="ts">
	import PianoRollControlLane from '../PianoRollControlLane.svelte';
	import {
		ADD_ROW_HEIGHT,
		LANE_HEIGHT,
		laneKey,
		type ControlLane
	} from '../../frontend/pianoRoll/controlLanes';
	import type { MidiControlPoint } from '../../types/global';

	export let lanes: ControlLane[];
	export let showAddRow: boolean;
	export let scrollLeft: number;
	export let controls: MidiControlPoint[];
	export let pxPerBeat: number;
	export let widthPx: number;
	export let color: string;
	export let quantize: number;
	export let commit: (next: MidiControlPoint[]) => Promise<unknown>;
</script>

<div class="lanes-wrapper">
	<div class="lanes-track" style="transform: translateX({-scrollLeft}px)">
		{#each lanes as lane (laneKey(lane))}
			<div class="lane-row" style="height: {LANE_HEIGHT}px">
				<PianoRollControlLane {lane} {controls} {pxPerBeat} {widthPx} {color} {quantize} {commit} />
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
