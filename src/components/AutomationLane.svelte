<script lang="ts">
	import { onDestroy } from 'svelte';
	import type { AutomationLane, AutomationPoint } from '../types/global';
	import { updateAutomationClip } from '../frontend/commands/dispatcher';
	import { ARRANGE_LANE_HEIGHT } from '../frontend/arrangeInteraction';
	import { curvePath, laneClipFor, lanePoints, valueToY, yToValue } from '../frontend/automation/lanes';
	import { notify } from '../stores/status';

	export let trackId: string;
	export let lane: AutomationLane;
	export let scale: number;
	export let widthPx: number;
	export let color: string;
	export let snapTime: (seconds: number, altKey: boolean) => number;

	const HIT_RADIUS_PX = 7;
	const height = ARRANGE_LANE_HEIGHT;

	let svg: SVGSVGElement;
	/** Points shown while dragging and until the engine echoes the edit back. */
	let draft: AutomationPoint[] | null = null;
	let dragIndex: number | null = null;
	let dragChanged = false;
	let echoedLane = lane;

	$: if (lane !== echoedLane) {
		echoedLane = lane;
		if (dragIndex === null) draft = null;
	}
	$: points = draft ?? lanePoints(lane);
	$: path = curvePath(points, lane.interpolation, scale, widthPx, height);

	function localPosition(e: MouseEvent): { x: number; y: number } {
		const rect = svg.getBoundingClientRect();
		return { x: e.clientX - rect.left, y: e.clientY - rect.top };
	}

	function pointAt(x: number, y: number): number | null {
		const index = points.findIndex(
			(p) => Math.hypot(p.time_s * scale - x, valueToY(p.value, height) - y) <= HIT_RADIUS_PX,
		);
		return index >= 0 ? index : null;
	}

	function commit(next: AutomationPoint[]) {
		draft = next;
		updateAutomationClip(trackId, lane.id, laneClipFor(lane, next)).catch((e) => {
			draft = null;
			notify(`Erreur automation : ${(e as Error)?.message ?? e}`, 'error');
		});
	}

	function onMouseDown(e: MouseEvent) {
		if (e.button !== 0) return;
		// The arrange behind would start a selection box.
		e.stopPropagation();
		const { x, y } = localPosition(e);
		const hit = pointAt(x, y);

		if (e.detail === 2) {
			if (hit !== null) commit(points.filter((_, i) => i !== hit));
			return;
		}

		if (hit === null) {
			const created = { time_s: snapTime(Math.max(0, x / scale), e.altKey), value: yToValue(y, height) };
			draft = [...points, created].sort((a, b) => a.time_s - b.time_s);
			dragIndex = draft.indexOf(created);
			dragChanged = true;
		} else {
			draft = [...points];
			dragIndex = hit;
			dragChanged = false;
		}
		window.addEventListener('mousemove', onMouseMove);
		window.addEventListener('mouseup', onMouseUp);
	}

	function onMouseMove(e: MouseEvent) {
		if (dragIndex === null || !draft) return;
		const { x, y } = localPosition(e);
		const previous = draft[dragIndex - 1];
		const following = draft[dragIndex + 1];
		let time = snapTime(Math.max(0, x / scale), e.altKey);
		if (previous) time = Math.max(time, previous.time_s);
		if (following) time = Math.min(time, following.time_s);
		draft = draft.map((p, i) => (i === dragIndex ? { time_s: time, value: yToValue(y, height) } : p));
		dragChanged = true;
	}

	function stopListening() {
		window.removeEventListener('mousemove', onMouseMove);
		window.removeEventListener('mouseup', onMouseUp);
	}

	function onMouseUp() {
		stopListening();
		const edited = draft;
		dragIndex = null;
		if (dragChanged && edited) commit(edited);
		else draft = null;
	}

	onDestroy(stopListening);
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<svg
	class="lane"
	bind:this={svg}
	width={widthPx}
	{height}
	style="--lane-color: {color}"
	on:mousedown={onMouseDown}
>
	<line class="separator" x1="0" y1="0.5" x2={widthPx} y2="0.5" />
	{#if path}
		<path class="area" d="{path} V {height} H 0 Z" />
		<path class="curve" d={path} />
	{/if}
	{#each points as point, i}
		<circle
			class="point"
			class:point--active={i === dragIndex}
			cx={point.time_s * scale}
			cy={valueToY(point.value, height)}
			r={i === dragIndex ? 4.5 : 3.5}
		/>
	{/each}
</svg>

<style lang="scss">
	@use '../variables.scss' as *;

	.lane {
		display: block;
		position: relative;
		background: rgba(255, 255, 255, 0.015);
	}

	.separator {
		stroke: $border-color;
		stroke-dasharray: 2 3;
	}

	.area {
		fill: var(--lane-color);
		opacity: 0.08;
	}

	.curve {
		fill: none;
		stroke: var(--lane-color);
		stroke-width: 1.5;
	}

	.point {
		fill: $background-color;
		stroke: var(--lane-color);
		stroke-width: 1.5;
		cursor: grab;

		&--active {
			fill: var(--lane-color);
			cursor: grabbing;
		}
	}
</style>
