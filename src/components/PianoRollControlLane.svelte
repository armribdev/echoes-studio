<!-- PianoRollControlLane.svelte — une ligne de courbe sous les notes : contrôle MIDI (pitch bend, CC, aftertouch) ou expression de la note choisie -->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import { snapBeat } from '../frontend/pianoRoll/coords';
	import {
		formatLaneValue,
		laneCentre,
		rampPoints,
		smoothRampPoints,
		replaceSpan,
		stepPath,
		strokePoints,
		valueToY,
		yToValue,
		type ControlLane,
		type LanePoint,
		type RampFunction
	} from '../frontend/pianoRoll/controlLanes';
	import type { LaneRange } from '../frontend/pianoRoll/noteExpressionLanes';
	import { notify } from '../stores/status';

	export let lane: ControlLane;
	export let height: number;
	/** The curve as the engine holds it; a new array means the engine echoed an edit back. */
	export let source: LanePoint[];
	/** Beats the points must stay in (the note of an expression lane); `null` for the whole clip. */
	export let range: LaneRange | null = null;
	export let editable = true;
	export let pxPerBeat: number;
	export let widthPx: number;
	export let color: string;
	export let quantize: number;
	export let commit: (next: LanePoint[]) => Promise<unknown>;

	const HIT_RADIUS_PX = 7;
	const STROKE_STEP_BEATS = 1 / 32;
	const RAMP_STEP_BEATS = 1 / 16;

	let svg: SVGSVGElement;
	/** Points shown while editing and until the engine echoes the edit back. */
	let draft: LanePoint[] | null = null;
	let echoedSource = source;
	let dragIndex: number | null = null;
	let dragChanged = false;
	let hoverIndex: number | null = null;
	let strokeBase: LanePoint[] = [];
	let strokeSamples: LanePoint[] | null = null;
	let rampAnchor: LanePoint | null = null;
	let rampShape: RampFunction = rampPoints;
	let stopListening: (() => void) | null = null;

	$: if (source !== echoedSource) {
		echoedSource = source;
		if (dragIndex === null && strokeSamples === null && rampAnchor === null) draft = null;
	}
	$: points = draft ?? source;
	$: path = stepPath(points, lane, pxPerBeat, range ? range.end * pxPerBeat : widthPx, height);
	$: centre = laneCentre(lane);
	$: labelIndex = strokeSamples || rampAnchor ? null : (dragIndex ?? hoverIndex);
	$: labelPoint = labelIndex !== null ? points[labelIndex] : undefined;

	function localPosition(e: MouseEvent): { x: number; y: number } {
		const rect = svg.getBoundingClientRect();
		return { x: Math.max(0, e.clientX - rect.left), y: e.clientY - rect.top };
	}

	function clampToRange(beat: number): number {
		return range ? Math.min(range.end, Math.max(range.start, beat)) : beat;
	}

	function rawBeatAt(x: number): number {
		return clampToRange(x / pxPerBeat);
	}

	function beatAt(x: number, altKey: boolean): number {
		const beat = x / pxPerBeat;
		return clampToRange(altKey || quantize <= 0 ? beat : snapBeat(beat, quantize));
	}

	function pointAt(x: number, y: number): number | null {
		const index = points.findIndex(
			(p) =>
				Math.hypot(p.beat * pxPerBeat - x, valueToY(p.value, lane, height) - y) <= HIT_RADIUS_PX
		);
		return index >= 0 ? index : null;
	}

	function save(next: LanePoint[]) {
		draft = next;
		commit(next).catch((e) => {
			draft = null;
			notify(`Erreur contrôles MIDI : ${(e as Error)?.message ?? e}`, 'error');
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

	function onMouseDown(e: MouseEvent) {
		if (e.button !== 0 || !editable) return;
		e.stopPropagation();
		const { x, y } = localPosition(e);
		const hit = pointAt(x, y);

		if (e.detail === 2) {
			if (hit !== null) save(points.filter((_, i) => i !== hit));
			return;
		}
		if (e.shiftKey) {
			startRamp(x, y, e.altKey, e.ctrlKey || e.metaKey ? smoothRampPoints : rampPoints);
			return;
		}
		if (e.ctrlKey || e.metaKey) {
			startStroke(x, y);
			return;
		}

		if (hit === null) {
			const created = { beat: beatAt(x, e.altKey), value: yToValue(y, lane, height) };
			draft = [...points, created].sort((a, b) => a.beat - b.beat);
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
		let beat = beatAt(x, e.altKey);
		if (previous) beat = Math.max(beat, previous.beat);
		if (following) beat = Math.min(beat, following.beat);
		draft = draft.map((p, i) => (i === dragIndex ? { beat, value: yToValue(y, lane, height) } : p));
		dragChanged = true;
	}

	function onDragEnd() {
		const edited = draft;
		dragIndex = null;
		if (dragChanged && edited) save(edited);
		else draft = null;
	}

	/** Ctrl + drag: free-hand, one value per step, over the points it crosses. */
	function startStroke(x: number, y: number) {
		strokeBase = points;
		strokeSamples = [{ beat: rawBeatAt(x), value: yToValue(y, lane, height) }];
		draft = replaceSpan(strokeBase, strokePoints(strokeSamples, STROKE_STEP_BEATS));
		listen(onStrokeMove, onStrokeEnd);
	}

	function onStrokeMove(e: MouseEvent) {
		if (!strokeSamples) return;
		const { x, y } = localPosition(e);
		const last = strokeSamples[strokeSamples.length - 1];
		const target = { beat: rawBeatAt(x), value: yToValue(y, lane, height) };
		const steps = Math.max(1, Math.ceil(Math.abs(target.beat - last.beat) / STROKE_STEP_BEATS));
		for (let step = 1; step <= steps; step++) {
			const t = step / steps;
			strokeSamples.push({
				beat: last.beat + t * (target.beat - last.beat),
				value: Math.round(last.value + t * (target.value - last.value))
			});
		}
		draft = replaceSpan(strokeBase, strokePoints(strokeSamples, STROKE_STEP_BEATS));
	}

	function onStrokeEnd() {
		const edited = draft;
		strokeSamples = null;
		if (edited) save(edited);
	}

	/** Shift + drag: a straight ramp from the press point to the pointer; with Ctrl, an eased one. */
	function startRamp(x: number, y: number, altKey: boolean, shape: RampFunction) {
		rampShape = shape;
		strokeBase = points;
		rampAnchor = { beat: beatAt(x, altKey), value: yToValue(y, lane, height) };
		draft = replaceSpan(strokeBase, [rampAnchor]);
		listen(onRampMove, onRampEnd);
	}

	function onRampMove(e: MouseEvent) {
		if (!rampAnchor) return;
		const { x, y } = localPosition(e);
		const target = { beat: beatAt(x, e.altKey), value: yToValue(y, lane, height) };
		draft = replaceSpan(strokeBase, rampShape(rampAnchor, target, RAMP_STEP_BEATS));
	}

	function onRampEnd() {
		const edited = draft;
		rampAnchor = null;
		if (edited) save(edited);
	}

	function onHover(e: MouseEvent) {
		if (dragIndex !== null || strokeSamples || rampAnchor) return;
		const { x, y } = localPosition(e);
		hoverIndex = pointAt(x, y);
	}

	onDestroy(() => stopListening?.());
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<svg
	class="lane"
	class:lane--over-point={hoverIndex !== null}
	class:lane--inert={!editable}
	bind:this={svg}
	width={widthPx}
	{height}
	style="--lane-color: {color}"
	on:mousedown={onMouseDown}
	on:mousemove={onHover}
	on:mouseleave={() => (hoverIndex = null)}
>
	<line class="separator" x1="0" y1="0.5" x2={widthPx} y2="0.5" />
	{#if range}
		<rect class="outside" x="0" y="0" width={range.start * pxPerBeat} {height} />
		<rect
			class="outside"
			x={range.end * pxPerBeat}
			y="0"
			width={Math.max(0, widthPx - range.end * pxPerBeat)}
			{height}
		/>
	{/if}
	{#if centre !== null}
		<line
			class="centre"
			x1="0"
			x2={widthPx}
			y1={valueToY(centre, lane, height)}
			y2={valueToY(centre, lane, height)}
		/>
	{/if}
	{#if path}
		<path class="area" d="{path} V {height} H 0 Z" />
		<path class="curve" d={path} />
	{/if}
	{#each points as point, i}
		<circle
			class="point"
			class:point--active={i === dragIndex}
			cx={point.beat * pxPerBeat}
			cy={valueToY(point.value, lane, height)}
			r={i === dragIndex || i === hoverIndex ? 4.5 : 3}
		/>
	{/each}
	{#if labelPoint}
		{@const cy = valueToY(labelPoint.value, lane, height)}
		<text class="value" x={labelPoint.beat * pxPerBeat + 8} y={cy < 14 ? cy + 14 : cy - 6}>
			{formatLaneValue(lane, labelPoint.value)}
		</text>
	{/if}
</svg>

<style lang="scss">
	@use '../variables.scss' as *;

	.lane {
		display: block;
		background: $surface-1;
		cursor: crosshair;

		&--over-point {
			cursor: grab;
		}

		&--inert {
			cursor: default;
		}
	}

	.outside {
		fill: $background-color;
		opacity: 0.6;
		pointer-events: none;
	}

	.separator {
		stroke: $border-color;
		stroke-dasharray: 2 3;
	}

	.centre {
		stroke: $border-color;
		stroke-width: 1;
		opacity: 0.6;
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
