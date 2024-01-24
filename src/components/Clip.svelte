<script lang="ts">
	export let start: number;
	export let end: number;
	export let scale: number;
	export let raw_samples: number[];
	export let sample_rate: number;

	let clip: HTMLElement;
	let canvas: HTMLCanvasElement;
	let context: CanvasRenderingContext2D | null;

	import { onMount } from 'svelte';

	onMount(() => {
		// drawWaveform();
	});

	function drawWaveform() {
		if (!canvas) return;
		context = canvas.getContext('2d');
		if (!context) return;

		canvas.width = clip.clientWidth;
		canvas.height = clip.clientHeight;
		context.clearRect(0, 0, canvas.width, canvas.height);

		context.strokeStyle = 'red';
		context.lineWidth = 1;
		context.beginPath();

		console.log(raw_samples);

		const startX = 0;
		const startY = (0.5 - raw_samples[0] / 2) * context.canvas.height;
		context.moveTo(startX, startY);
		console.log(startX, startY);

		for (let i = 1; i < raw_samples.length; i++) {
			const x = (i * scale) / sample_rate;
			const y = (0.5 - raw_samples[i] / 2) * context.canvas.height;
			context.lineTo(x, y);
		}

		context.stroke();
	}
</script>

<div
	bind:this={clip}
	class="clip"
	style="left: {start * scale}px; width: {(end - start) * scale}px"
>
	<canvas bind:this={canvas}></canvas>
</div>

<style lang="scss">
	.clip {
		position: absolute;
		display: block;
		background-color: pink;
		border-radius: 5px;
		top: 0;
		bottom: 0;
	}
</style>
