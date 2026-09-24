<script lang="ts">
	import { createEventDispatcher, onDestroy } from 'svelte';
	import { isTauri } from '@tauri-apps/api/core';
	import { getCurrentWindow, LogicalPosition } from '@tauri-apps/api/window';
	import { modulationDrag } from '../stores/modulationDrag';

	export let value: number;
	export let min: number;
	export let max: number;
	export let pixelRange: number = 200;
	export let defaultValue: number | undefined = undefined;
	/** Overrides the value-arc color, e.g. to match this control's modulator source. */
	export let color: string | undefined = undefined;
	/** Modulator color to draw as a ring when this parameter is an active modulation target. */
	export let modColor: string | undefined = undefined;
	/** Modulation target key; makes this knob a drop target for dragged modulators. */
	export let modTargetKey: string | undefined = undefined;

	$: dropCandidate = modTargetKey !== undefined && $modulationDrag !== null;
	$: dropHover = dropCandidate && $modulationDrag?.hoverKey === modTargetKey;

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
	const RING_R = 12;
	const RING_STROKE = 1.25;

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
	// In Tauri: hide cursor + warp back to origin each move → infinite drag, no popup.
	// In browser dev: absolute drag fallback (limited by screen edges).

	let knobEl: HTMLDivElement;
	let dragging = false;
	let warpDrag = false;
	let originX = 0;
	let originY = 0;
	let startY = 0;
	let startValue = 0;
	let dragDeltaY = 0;
	let captureEl: HTMLElement | null = null;
	let capturePointerId = -1;

	function clamp(n: number, lo: number, hi: number): number {
		return Math.max(lo, Math.min(hi, n));
	}

	function applyDrag(deltaY: number): void {
		value = clamp(startValue - ((max - min) * deltaY) / pixelRange, min, max);
	}

	function warpCursorToOrigin(): void {
		void getCurrentWindow()
			.setCursorPosition(new LogicalPosition(originX, originY))
			.catch(() => {});
	}

	function pointerMove(e: PointerEvent): void {
		if (warpDrag) {
			dragDeltaY += e.clientY - originY;
			applyDrag(dragDeltaY);
			warpCursorToOrigin();
		} else {
			applyDrag(e.clientY - startY);
		}
	}

	function removeDragListeners(): void {
		window.removeEventListener('pointermove', pointerMove);
		window.removeEventListener('pointerup', pointerUp);
		window.removeEventListener('pointercancel', pointerUp);
	}

	async function hideCursor(): Promise<void> {
		document.body.style.cursor = 'none';
		if (!warpDrag) return;
		try {
			await getCurrentWindow().setCursorVisible(false);
		} catch {
			// Unavailable platform API.
		}
	}

	async function restoreCursor(): Promise<void> {
		document.body.style.cursor = '';
		if (!warpDrag) return;
		try {
			const win = getCurrentWindow();
			await win.setCursorPosition(new LogicalPosition(originX, originY));
			await win.setCursorVisible(true);
		} catch {
			// Browser dev: CSS cursor restore is enough.
		}
	}

	function finishDrag(): void {
		if (!dragging) return;
		dragging = false;
		removeDragListeners();
		if (captureEl && capturePointerId >= 0) {
			captureEl.releasePointerCapture(capturePointerId);
			captureEl = null;
			capturePointerId = -1;
		}
		void restoreCursor().then(() => {
			dispatch('release', value);
		});
	}

	function pointerUp(): void {
		finishDrag();
	}

	function doubleClick(): void {
		const reset = defaultValue !== undefined ? defaultValue : (min + max) / 2;
		value = reset;
		dispatch('release', value);
	}

	function pointerDown(e: PointerEvent): void {
		e.preventDefault();
		if (dragging) return;

		captureEl = knobEl;
		capturePointerId = e.pointerId;
		knobEl.setPointerCapture(e.pointerId);

		warpDrag = isTauri();
		dragging = true;
		dragDeltaY = 0;
		originX = e.clientX;
		originY = e.clientY;
		startY = e.clientY;
		startValue = value;

		void hideCursor();

		window.addEventListener('pointermove', pointerMove);
		window.addEventListener('pointerup', pointerUp);
		window.addEventListener('pointercancel', pointerUp);
	}

	onDestroy(() => {
		if (dragging) finishDrag();
	});
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	class="knob"
	class:dragging
	class:drop-candidate={dropCandidate}
	class:drop-hover={dropHover}
	style:--drop-color={$modulationDrag?.color}
	data-mod-target={modTargetKey}
	bind:this={knobEl}
	on:pointerdown={pointerDown}
	on:dblclick={doubleClick}
>
	<svg width={SIZE} height={SIZE} viewBox="0 0 {SIZE} {SIZE}" overflow="visible">
		{#if modColor}
			<circle class="mod-ring" cx={CX} cy={CY} r={RING_R} stroke-width={RING_STROKE} stroke={modColor} />
		{/if}
		<path class="track" d={trackPath} stroke-width={STROKE} />
		{#if valuePath}
			<path class="value-arc" d={valuePath} stroke-width={STROKE} stroke={color} />
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

		&.dragging {
			cursor: none;
		}

		&.drop-candidate {
			box-shadow: 0 0 0 1px color-mix(in srgb, var(--drop-color) 45%, transparent);
		}

		&.drop-hover {
			box-shadow: 0 0 0 2px var(--drop-color);
			background: color-mix(in srgb, var(--drop-color) 20%, transparent);
			filter: brightness(1.4);
		}
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

	.mod-ring {
		fill: none;
		opacity: 0.65;
		stroke-dasharray: 2 2;
	}

</style>
