<script lang="ts">
	import { onDestroy } from 'svelte';
	import type { AutomationLane, AutomationPoint, TrackEffect } from '../types/global';
	import { updateAutomationClip } from '../frontend/commands/dispatcher';
	import { ARRANGE_LANE_HEIGHT } from '../frontend/arrangeInteraction';
	import {
		curvePath,
		automationRange,
		formatAutomationValue,
		laneClipFor,
		lanePoints,
		simplifyPolyline,
		toPlainValue,
		valueToY,
		yToValue
	} from '../frontend/automation/lanes';
	import {
		cachedPluginParamText,
		pluginParamText,
		pluginParamsByPlacement
	} from '../stores/pluginParams';
	import { notify } from '../stores/status';

	export let trackId: string;
	export let lane: AutomationLane;
	export let effects: TrackEffect[];
	export let scale: number;
	export let widthPx: number;
	export let color: string;
	export let snapTime: (seconds: number, altKey: boolean) => number;

	const HIT_RADIUS_PX = 7;
	/** Each text request briefly holds the plugin, so a drag asks at this pace. */
	const PARAM_TEXT_INTERVAL_MS = 100;
	const PENCIL_TOLERANCE_PX = 1.5;
	const height = ARRANGE_LANE_HEIGHT;

	let svg: SVGSVGElement;
	/** Points shown while editing and until the engine echoes the edit back. */
	let draft: AutomationPoint[] | null = null;
	let dragIndex: number | null = null;
	let dragChanged = false;
	let hoverIndex: number | null = null;
	/** Pencil stroke: y in pixels per x pixel column. */
	let stroke: Map<number, number> | null = null;
	let strokeBase: AutomationPoint[] = [];
	let strokeLast = { x: 0, y: 0 };
	let echoedLane = lane;

	$: if (lane !== echoedLane) {
		echoedLane = lane;
		if (dragIndex === null && stroke === null) draft = null;
	}
	$: points = draft ?? lanePoints(lane);
	$: path = curvePath(points, lane.interpolation, scale, widthPx, height);
	$: range = automationRange(lane.target, effects, $pluginParamsByPlacement);
	$: labelIndex = stroke ? null : (dragIndex ?? hoverIndex);
	$: labelPoint = labelIndex !== null ? points[labelIndex] : undefined;
	$: plainValue = labelPoint && range ? toPlainValue(labelPoint.value, range) : null;
	$: scheduleParamText(lane.target, plainValue);
	$: shownText =
		paramText && (dragIndex !== null || paramText.plain === plainValue) ? paramText.text : null;

	let paramText: { plain: number; text: string } | null = null;
	let paramTextTimer: ReturnType<typeof setTimeout> | undefined;
	let paramTextAskedAt = 0;

	function scheduleParamText(target: AutomationLane['target'], plain: number | null) {
		clearTimeout(paramTextTimer);
		if (target.kind !== 'plugin_param' || plain === null) {
			paramText = null;
			return;
		}
		const { effect_id: placementKey, native_id: nativeId } = target;
		const cached = cachedPluginParamText(placementKey, nativeId, plain);
		if (cached !== undefined) {
			paramText = cached === null ? null : { plain, text: cached };
			return;
		}
		const wait = Math.max(0, paramTextAskedAt + PARAM_TEXT_INTERVAL_MS - Date.now());
		paramTextTimer = setTimeout(async () => {
			paramTextAskedAt = Date.now();
			const text = await pluginParamText(placementKey, nativeId, plain);
			if (plain === plainValue) paramText = text === null ? null : { plain, text };
		}, wait);
	}

	function localPosition(e: MouseEvent): { x: number; y: number } {
		const rect = svg.getBoundingClientRect();
		return { x: Math.max(0, e.clientX - rect.left), y: e.clientY - rect.top };
	}

	function pointAt(x: number, y: number): number | null {
		const index = points.findIndex(
			(p) => Math.hypot(p.time_s * scale - x, valueToY(p.value, height) - y) <= HIT_RADIUS_PX
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

	function listen(onMove: (e: MouseEvent) => void, onUp: () => void) {
		const stop = () => {
			window.removeEventListener('mousemove', onMove);
			window.removeEventListener('mouseup', up);
			stopListening = null;
		};
		const up = () => {
			stop();
			onUp();
		};
		window.addEventListener('mousemove', onMove);
		window.addEventListener('mouseup', up);
		stopListening = stop;
	}
	let stopListening: (() => void) | null = null;

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
		if (e.ctrlKey || e.metaKey) {
			startStroke(x, y);
			return;
		}

		if (hit === null) {
			const created = { time_s: snapTime(x / scale, e.altKey), value: yToValue(y, height) };
			draft = [...points, created].sort((a, b) => a.time_s - b.time_s);
			dragIndex = draft.indexOf(created);
			dragChanged = true;
		} else {
			draft = [...points];
			dragIndex = hit;
			dragChanged = false;
		}
		listen(onDragMove, onDragEnd);
	}

	function onDragMove(e: MouseEvent) {
		if (dragIndex === null || !draft) return;
		const { x, y } = localPosition(e);
		const previous = draft[dragIndex - 1];
		const following = draft[dragIndex + 1];
		let time = snapTime(x / scale, e.altKey);
		if (previous) time = Math.max(time, previous.time_s);
		if (following) time = Math.min(time, following.time_s);
		draft = draft.map((p, i) =>
			i === dragIndex ? { time_s: time, value: yToValue(y, height) } : p
		);
		dragChanged = true;
	}

	function onDragEnd() {
		const edited = draft;
		dragIndex = null;
		if (dragChanged && edited) commit(edited);
		else draft = null;
	}

	function startStroke(x: number, y: number) {
		strokeBase = points;
		strokeLast = { x: Math.round(x), y };
		stroke = new Map([[strokeLast.x, y]]);
		draft = strokeDraft(stroke);
		listen(onStrokeMove, onStrokeEnd);
	}

	function onStrokeMove(e: MouseEvent) {
		if (!stroke) return;
		const { x, y } = localPosition(e);
		const column = Math.round(x);
		const steps = Math.abs(column - strokeLast.x);
		for (let step = 1; step <= steps; step++) {
			const t = step / steps;
			stroke.set(
				strokeLast.x + Math.sign(column - strokeLast.x) * step,
				strokeLast.y + t * (y - strokeLast.y)
			);
		}
		if (steps === 0) stroke.set(column, y);
		strokeLast = { x: column, y };
		draft = strokeDraft(stroke);
	}

	/** The drawn span replaces the points under it; the rest of the lane is kept. */
	function strokeDraft(columns: Map<number, number>): AutomationPoint[] {
		const drawn = [...columns.entries()].sort((a, b) => a[0] - b[0]).map(([x, y]) => ({ x, y }));
		const from = drawn[0].x / scale;
		const to = drawn[drawn.length - 1].x / scale;
		const kept = strokeBase.filter((p) => p.time_s < from || p.time_s > to);
		const added = simplifyPolyline(drawn, PENCIL_TOLERANCE_PX).map((p) => ({
			time_s: p.x / scale,
			value: yToValue(p.y, height)
		}));
		return [...kept, ...added].sort((a, b) => a.time_s - b.time_s);
	}

	function onStrokeEnd() {
		const edited = draft;
		stroke = null;
		if (edited) commit(edited);
	}

	function onHover(e: MouseEvent) {
		if (dragIndex !== null || stroke) return;
		const { x, y } = localPosition(e);
		hoverIndex = pointAt(x, y);
	}

	onDestroy(() => {
		stopListening?.();
		clearTimeout(paramTextTimer);
	});
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<svg
	class="lane"
	class:lane--over-point={hoverIndex !== null}
	bind:this={svg}
	width={widthPx}
	{height}
	style="--lane-color: {color}"
	on:mousedown={onMouseDown}
	on:mousemove={onHover}
	on:mouseleave={() => (hoverIndex = null)}
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
			r={i === dragIndex || i === hoverIndex ? 4.5 : 3.5}
		/>
	{/each}
	{#if labelPoint}
		{@const cy = valueToY(labelPoint.value, height)}
		<text class="value" x={labelPoint.time_s * scale + 8} y={cy < 16 ? cy + 14 : cy - 7}>
			{shownText ?? formatAutomationValue(labelPoint.value, range)}
		</text>
	{/if}
</svg>

<style lang="scss">
	@use '../variables.scss' as *;

	.lane {
		display: block;
		position: relative;
		background: rgba(255, 255, 255, 0.015);

		&--over-point {
			cursor: grab;
		}
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

		&--active {
			fill: var(--lane-color);
		}
	}

	.value {
		font-size: 10px;
		font-family: $font-mono;
		fill: $text-color;
		stroke: $background-color;
		stroke-width: 3;
		paint-order: stroke;
		pointer-events: none;
		user-select: none;
	}
</style>
