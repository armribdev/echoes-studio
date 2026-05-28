<script lang="ts">
	import { createEventDispatcher } from 'svelte';

	export let value: number;
	export let min: number;
	export let max: number;
	export let pixelRange: number = 200;
	export let defaultValue: number | undefined = undefined;

	const dispatch = createEventDispatcher<{ release: number }>();

	// ── Arc geometry ──────────────────────────────────────────────────────────
	// The knob sweeps 298.8° (≈300°), starting at 7:30 and ending at 4:30,
	// going clockwise through 12 o'clock.
	//
	// CSS rotate convention: 0 = up (12 o'clock), clockwise positive.
	// Math/SVG x-axis convention: 0 = right (3 o'clock), CCW positive,
	//   but SVG y-axis is flipped, so clockwise in screen = decreasing math angle.
	//
	// Conversion: mathAngle = π/2 − cssRot

	const SIZE = 26;
	const CX = SIZE / 2;
	const CY = SIZE / 2;
	const TRACK_R = 9;
	const STROKE = 2.5;

	const START_CSS = -Math.PI * 0.83;
	const ROT_RANGE = 2 * Math.PI * 0.83;
	const END_CSS = START_CSS + ROT_RANGE;

	function cssToMath(cssRad: number): number {
		return Math.PI / 2 - cssRad;
	}

	const START_MATH = cssToMath(START_CSS);
	const END_MATH = cssToMath(END_CSS);

	function polar(angleMath: number, r: number) {
		return {
			x: CX + r * Math.cos(angleMath),
			y: CY - r * Math.sin(angleMath),
		};
	}

	// Generates an SVG arc going clockwise from fromMath to toMath.
	function arcPath(fromMath: number, toMath: number, r: number): string {
		const s = polar(fromMath, r);
		const e = polar(toMath, r);
		let sweep = fromMath - toMath;
		while (sweep < 0) sweep += 2 * Math.PI;
		while (sweep >= 2 * Math.PI) sweep -= 2 * Math.PI;
		const large = sweep > Math.PI ? 1 : 0;
		return `M${s.x.toFixed(3)},${s.y.toFixed(3)} A${r},${r},0,${large},1,${e.x.toFixed(3)},${e.y.toFixed(3)}`;
	}

	$: t = Math.max(0, Math.min(1, (value - min) / (max - min)));
	$: valueMath = cssToMath(START_CSS + t * ROT_RANGE);
	$: trackPath = arcPath(START_MATH, END_MATH, TRACK_R);
	$: valuePath = t > 0.005 ? arcPath(START_MATH, valueMath, TRACK_R) : '';

	// ── Drag interaction ──────────────────────────────────────────────────────
	let startY: number, startValue: number;

	function clamp(n: number, lo: number, hi: number): number {
		return Math.max(lo, Math.min(hi, n));
	}

	function pointerMove(e: PointerEvent): void {
		value = clamp(startValue - ((max - min) * (e.clientY - startY)) / pixelRange, min, max);
	}

	function pointerUp(): void {
		document.body.style.cursor = '';
		window.removeEventListener('pointermove', pointerMove);
		window.removeEventListener('pointerup', pointerUp);
		dispatch('release', value);
	}

	function doubleClick(): void {
		const reset = defaultValue !== undefined ? defaultValue : (min + max) / 2;
		value = reset;
		dispatch('release', value);
	}

	function pointerDown(e: PointerEvent): void {
		e.preventDefault();
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		document.body.style.cursor = 'ns-resize';
		startY = e.clientY;
		startValue = value;
		window.addEventListener('pointermove', pointerMove);
		window.addEventListener('pointerup', pointerUp);
	}
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div class="knob" on:pointerdown={pointerDown} on:dblclick={doubleClick}>
	<svg width={SIZE} height={SIZE} viewBox="0 0 {SIZE} {SIZE}">
		<path class="track" d={trackPath} stroke-width={STROKE} />
		{#if valuePath}
			<path class="value-arc" d={valuePath} stroke-width={STROKE} />
		{/if}
	</svg>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.knob {
		width: 26px;
		height: 26px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		cursor: ns-resize;
		border-radius: 50%;
		transition: filter 0.1s;

		&:hover { filter: brightness(1.25); }
		&:active { filter: brightness(1.5); }
	}

	svg {
		display: block;
		overflow: visible;
	}

	.track {
		fill: none;
		stroke: $border-color;
		stroke-linecap: round;
	}

	.value-arc {
		fill: none;
		stroke: $accent-color;
		stroke-linecap: round;
	}

</style>
