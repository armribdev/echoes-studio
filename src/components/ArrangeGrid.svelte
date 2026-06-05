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

	function redraw() {
		if (!canvas || widthPx <= 0 || heightPx <= 0) return;

		const w = Math.ceil(widthPx * dpr);
		const h = Math.ceil(heightPx * dpr);
		if (canvas.width !== w || canvas.height !== h) {
			canvas.width = w;
			canvas.height = h;
		}

		const ctx = canvas.getContext('2d');
		if (!ctx) return;

		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		drawArrangeGrid({
			ctx,
			width: widthPx,
			height: heightPx,
			pxPerBeat,
			numerator,
			totalBeats,
			gridBeats,
		});
	}

	$: widthPx, heightPx, pxPerBeat, numerator, totalBeats, gridBeats, canvas, redraw();

	onMount(redraw);
</script>

<canvas
	class="arrange-grid"
	bind:this={canvas}
	style="width: {widthPx}px; height: {heightPx}px"
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
