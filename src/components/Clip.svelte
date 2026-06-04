<script lang="ts">
	import { onMount, createEventDispatcher } from 'svelte';
	import { selection } from '../stores/selection';
	import { keyboardFocus } from '../stores/keyboardFocus';

	export let id: string;
	export let start: number;
	export let end: number;
	export let offset: number = 0;
	export let scale: number;
	export let sample_rate: number;
	export let peaks_64: [number, number][];
	export let peaks_512: [number, number][];
	export let peaks_4096: [number, number][];

	const dispatch = createEventDispatcher<{
		clipdragstart: { offsetPx: number };
		clipresizestart: { edge: 'left' | 'right'; clientX: number };
		clipcontextmenu: { id: string; x: number; y: number };
	}>();

	let clipEl: HTMLElement;
	let canvas: HTMLCanvasElement;
	let syncWorker: Worker;
	let workerReady = false;
	const bitmapCache = new Map<string, ImageBitmap>();
	let latestKey = '';

	// Not reactive — accessed only inside functions so Svelte doesn't track them as $: dependencies
	let _lastW = 0;
	let _lastKey = '';
	let _drawnBitmap: ImageBitmap | null = null;

	$: selected = $selection.ids.has(id);

	function scaleBucket(s: number): number {
		return Math.round(Math.log2(s) * 8);
	}

	// Cache/render key: same zoom bucket + same trim offset → same waveform pixels.
	function renderKey(bucket: number, off: number): string {
		return `${bucket}|${off.toFixed(6)}`;
	}

	// Returns true (and updates tracking) only when canvas size, zoom level, or trim
	// offset actually changed. Called from a $: block — using a function means Svelte
	// won't track _lastW/_lastKey as reactive dependencies, so drag frames that change
	// only `start` (a plain move) are skipped entirely.
	function needsRender(w: number, key: string): boolean {
		if (w === _lastW && key === _lastKey) return false;
		_lastW = w;
		_lastKey = key;
		return true;
	}

	const loadWorker = async () => {
		const SyncWorker = await import('$lib/waveform.worker?worker');
		syncWorker = new SyncWorker.default();
		syncWorker.onmessage = ({ data }: MessageEvent<{ bitmap: ImageBitmap; key: string }>) => {
			bitmapCache.set(data.key, data.bitmap);
			if (data.key === latestKey) drawBitmap(data.bitmap);
		};
		workerReady = true;
	};

	function drawBitmap(bitmap: ImageBitmap) {
		if (!canvas) return;
		// Skip if this exact bitmap is already drawn at the correct size — avoids clearRect/drawImage
		// thrashing during drag frames, which causes GPU compositing artefacts in WebView2.
		if (_drawnBitmap === bitmap && canvas.width === bitmap.width) return;
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		if (canvas.width !== bitmap.width) canvas.width = bitmap.width;
		ctx.clearRect(0, 0, canvas.width, canvas.height);
		ctx.drawImage(bitmap, 0, 0);
		_drawnBitmap = bitmap;
	}

	function requestRender(targetWidth: number) {
		if (!syncWorker || !canvas.height) return;
		const bucket = scaleBucket(scale);
		const key = renderKey(bucket, offset);
		latestKey = key;
		const cached = bitmapCache.get(key);
		if (cached && cached.width === targetWidth) {
			drawBitmap(cached);
			return;
		}
		syncWorker.postMessage({
			peaks_64,
			peaks_512,
			peaks_4096,
			sample_rate,
			scale,
			width: targetWidth,
			height: canvas.height,
			key,
			offset
		});
	}

	onMount(() => {
		canvas.width = Math.max(1, Math.ceil((end - start) * scale));
		canvas.height = clipEl.clientHeight;
		loadWorker();
	});

	// Fires when scale, end, or start changes — and when workerReady first becomes true.
	// needsRender() filters out drag frames where only position (start/end) changed,
	// not the canvas dimensions or zoom level.
	$: if (canvas && workerReady) {
		const w = Math.max(1, Math.ceil((end - start) * scale));
		const key = renderKey(scaleBucket(scale), offset);
		if (needsRender(w, key)) requestRender(w);
	}

	function onContextMenu(e: MouseEvent) {
		e.preventDefault();
		e.stopPropagation();
		dispatch('clipcontextmenu', { id, x: e.clientX, y: e.clientY });
	}

	function onKeyDown(e: KeyboardEvent) {
		if (e.key !== 'Enter' && e.key !== ' ') return;
		e.preventDefault();
		e.stopPropagation();
	}

	function onMouseDown(e: MouseEvent) {
		e.stopPropagation();
		e.preventDefault();

		keyboardFocus.setTimeline();

		selection.select(id, { additive: e.ctrlKey || e.metaKey });

		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		dispatch('clipdragstart', { offsetPx: e.clientX - rect.left });
	}

	function onResizeHandleDown(edge: 'left' | 'right', e: MouseEvent) {
		e.stopPropagation();
		e.preventDefault();
		keyboardFocus.setTimeline();
		selection.select(id, { additive: e.ctrlKey || e.metaKey });
		dispatch('clipresizestart', { edge, clientX: e.clientX });
	}
</script>

<div
	bind:this={clipEl}
	class="clip"
	class:selected
	role="button"
	tabindex="0"
	style="transform: translateX({start * scale}px); width: {(end - start) * scale}px"
	on:mousedown={onMouseDown}
	on:contextmenu={onContextMenu}
	on:click|stopPropagation
	on:keydown={onKeyDown}
>
	<canvas bind:this={canvas}></canvas>
	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div class="resize-handle resize-handle--left" on:mousedown={(e) => onResizeHandleDown('left', e)}></div>
	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div class="resize-handle resize-handle--right" on:mousedown={(e) => onResizeHandleDown('right', e)}></div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.clip {
		position: absolute;
		display: block;
		background-color: #1f3c72;
		box-shadow: inset 0 0 0 1px rgba(21, 113, 255, 0.45);
		border-radius: 4px;
		top: 0;
		bottom: 0;
		overflow: hidden;
		cursor: grab;

		&:active {
			cursor: grabbing;
		}

		&.selected {
			background-color: #1a4fa0;
		}

		canvas {
			display: block;
			background-color: inherit;
		}

		.resize-handle {
			position: absolute;
			top: 0;
			bottom: 0;
			width: 6px;
			cursor: ew-resize;
			z-index: 2;

			&--left {
				left: 0;
			}

			&--right {
				right: 0;
			}
		}
	}
</style>
