<script lang="ts">
	import { onMount } from 'svelte';

	export let start: number;
	export let end: number;
	export let scale: number;
	export let raw_samples: number[];
	export let sample_rate: number;

	let clip: HTMLElement;
	let canvas: HTMLCanvasElement;

	let syncWorker: Worker;

	const loadWorker = async () => {
		const SyncWorker = await import('$lib/waveform.worker?worker');
		syncWorker = new SyncWorker.default();
	};

	onMount(() => {
		if (!canvas) return;
		canvas.width = clip.clientWidth;
		canvas.height = clip.clientHeight;

		loadWorker().then(() =>
			syncWorker.postMessage({
				raw_samples: raw_samples,
				sample_rate: sample_rate,
				scale: scale
			})
		);
	});
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
		overflow: hidden;
		cursor: grab;
	}
</style>
