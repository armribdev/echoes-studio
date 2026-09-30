<script lang="ts">
	import { onMount } from 'svelte';
	import { drawArrangeGrid } from '../frontend/arrange/gridRenderer';

	export let widthPx: number;
	export let heightPx: number;
	export let pxPerBeat: number;
	export let numerator: number;
	export let totalBeats: number;
	export let gridBeats: number;

	let canvas: HTMLCanvasElement;
	const dpr = typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1;

	let cssW = 0;
	let cssH = 0;

	function redraw() {
		if (!canvas || widthPx <= 0 || heightPx <= 0) return;

		// Buffer sized so CSS width = buffer/dpr exactly → no browser stretch/blur.
		const w = Math.max(1, Math.round(widthPx * dpr));
		const h = Math.max(1, Math.round(heightPx * dpr));
		cssW = w / dpr;
		cssH = h / dpr;

		if (canvas.width !== w || canvas.height !== h) {
			canvas.width = w;
			canvas.height = h;
		}

		const ctx = canvas.getContext('2d');
		if (!ctx) return;

		ctx.setTransform(1, 0, 0, 1, 0, 0);
		drawArrangeGrid({
			ctx,
			width: w,
			height: h,
			pxPerBeat,
			dpr,
			numerator,
			totalBeats,
			gridBeats
		});
	}

	$: (widthPx, heightPx, pxPerBeat, numerator, totalBeats, gridBeats, canvas, redraw());

	onMount(redraw);
</script>

<canvas
	class="arrange-grid"
	bind:this={canvas}
	style="width: {cssW}px; height: {cssH}px"
	aria-hidden="true"
></canvas>

<style lang="scss">
	.arrange-grid {
		position: absolute;
		top: 0;
		left: 0;
		pointer-events: none;
		z-index: 0;
	}
</style>
