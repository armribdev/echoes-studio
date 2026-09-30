<script lang="ts">
	import { onMount, onDestroy, createEventDispatcher } from 'svelte';
	import { selection } from '../stores/selection';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import { arrangeClipFrame } from '../frontend/arrangeInteraction';
	import { importMark } from '../frontend/audio/importDebugMarks';

	export let id: string;
	export let start: number;
	export let end: number;
	export let offset: number = 0;
	export let scale: number;
	export let sample_rate: number;
	export let peaks_64: [number, number][];
	export let peaks_512: [number, number][];
	export let peaks_4096: [number, number][];
	/** Optimistic import preview — waveform may stream in; drag still allowed. */
	export let pending: boolean = false;
	/** 0..1 decode progress (kept for API; coverage is driven by peak lanes). */
	export let loadProgress: number = 1;
	/** Stable pending-import id — used to signal handoff paint without remount flash. */
	export let pendingImportId: string | null = null;

	const WAVE_FILL = 'rgba(71, 171, 255, 0.85)';

	const dispatch = createEventDispatcher<{
		clipdragstart: { offsetPx: number };
		clipresizestart: { edge: 'left' | 'right'; clientX: number };
		clipcontextmenu: { id: string; x: number; y: number };
		pendingpainted: { pendingImportId: string };
	}>();

	let clipEl: HTMLElement;
	let canvas: HTMLCanvasElement;
	let syncWorker: Worker;
	let workerReady = false;
	const bitmapCache = new Map<string, ImageBitmap>();
	let latestKey = '';
	let wasPending = pending;
	let paintSignaled = false;
	/** After import handoff, keep the grow canvas until zoom/offset changes (avoids worker clear flash). */
	let holdWorkerAfterHandoff = false;
	let holdScaleBucket = 0;
	let holdWidth = 0;
	let holdPeaksEpoch = '';

	let _lastW = 0;
	let _lastKey = '';
	let _drawnBitmap: ImageBitmap | null = null;

	// Pending: same fillRect columns as the worker, growing 0→target L→R.
	let _head = 0;
	let _raf = 0;
	let _goalTops = new Float32Array(0);
	let _goalBots = new Float32Array(0);
	let _showTops = new Float32Array(0);
	let _showBots = new Float32Array(0);
	let _grow = new Float32Array(0);
	let _colsDirty = true;
	let _peaksSig = '';
	// Coasting coverage: rate from batch intervals, smoothCovered bridges gaps between lots.
	let _loadRate = 0;
	let _coastSpeed = 0;
	let _smoothCovered = 0;
	let _rawCovered = 0;
	let _lastBatchAt = 0;
	let _lastTs = 0;

	$: selected = $selection.ids.has(id);
	$: frame = arrangeClipFrame(start, end, scale);
	$: canvasCssW = Math.max(1, Math.round(frame.widthPx));
	$: peaksEpoch = `${peaks_64.length}:${peaks_512.length}:${peaks_4096.length}`;
	$: void loadProgress;

	function scaleBucket(s: number): number {
		return Math.round(Math.log2(s) * 8);
	}

	function renderKey(bucket: number, off: number, epoch: string): string {
		return `${bucket}|${off.toFixed(6)}|${epoch}`;
	}

	function needsRender(w: number, key: string): boolean {
		if (w === _lastW && key === _lastKey) return false;
		_lastW = w;
		_lastKey = key;
		return true;
	}

	function selectPeaks(
		p64: [number, number][],
		p512: [number, number][],
		p4096: [number, number][],
		sr: number,
		s: number,
		streamedOnly = false
	): { peaks: [number, number][]; chunkSize: number } {
		const samplesPerPixel = sr / s;
		// Streamed-only: ignore peaks_64 (arrives at decode finalize and would pop).
		if (!streamedOnly && !pending) {
			if (samplesPerPixel < 512 && p64.length > 0) return { peaks: p64, chunkSize: 64 };
		}
		if (samplesPerPixel < 4096 && p512.length > 0) return { peaks: p512, chunkSize: 512 };
		if (p4096.length > 0) return { peaks: p4096, chunkSize: 4096 };
		if (p512.length > 0) return { peaks: p512, chunkSize: 512 };
		return { peaks: p64, chunkSize: 64 };
	}

	/** Same column mapping as waveform.worker — identical final look. */
	function rebuildGoals(width: number, height: number, streamedOnly = false) {
		const prevGrow = _grow;
		const prevShowT = _showTops;
		const prevShowB = _showBots;
		const prevW = prevGrow.length;
		const goalsT = new Float32Array(width);
		const goalsB = new Float32Array(width);
		const showT = new Float32Array(width);
		const showB = new Float32Array(width);
		const grow = new Float32Array(width);
		const mid = height * 0.5;
		goalsT.fill(mid);
		goalsB.fill(mid);
		showT.fill(mid);
		showB.fill(mid);

		const { peaks, chunkSize } = selectPeaks(
			peaks_64,
			peaks_512,
			peaks_4096,
			sample_rate,
			scale,
			streamedOnly || pending
		);

		if (peaks.length > 0 && sample_rate > 0) {
			const secPerChunk = chunkSize / sample_rate;
			for (let px = 0; px < width; px++) {
				const timeStart = offset + px / scale;
				const timeEnd = offset + (px + 1) / scale;
				const chunkStart = Math.floor(timeStart / secPerChunk);
				const chunkEnd = Math.floor(timeEnd / secPerChunk);
				const limit = Math.min(Math.max(chunkStart + 1, chunkEnd + 1), peaks.length);

				let pMin = 0;
				let pMax = 0;
				for (let c = chunkStart; c < limit; c++) {
					const [lo, hi] = peaks[c];
					if (lo < pMin) pMin = lo;
					if (hi > pMax) pMax = hi;
				}

				goalsT[px] = (0.5 - pMax * 0.5) * height;
				goalsB[px] = (0.5 - pMin * 0.5) * height;
			}
		}

		const copy = Math.min(prevW, width);
		for (let i = 0; i < copy; i++) {
			grow[i] = prevGrow[i];
			showT[i] = prevShowT[i] ?? mid;
			showB[i] = prevShowB[i] ?? mid;
		}

		_goalTops = goalsT;
		_goalBots = goalsB;
		_showTops = showT;
		_showBots = showB;
		_grow = grow;
		_colsDirty = false;
	}

	function peaksCoveragePx(width: number): number {
		const { peaks, chunkSize } = selectPeaks(
			peaks_64,
			peaks_512,
			peaks_4096,
			sample_rate,
			scale,
			true
		);
		if (peaks.length === 0 || sample_rate <= 0 || scale <= 0) return 0;
		const coveredSec = (peaks.length * chunkSize) / sample_rate;
		return Math.max(0, Math.min(width, (coveredSec - offset) * scale));
	}

	/** Worker-identical fillRect, using morphed amplitudes × grow. */
	function paintGrowingColumns(ctx: CanvasRenderingContext2D) {
		const W = canvas.width;
		const H = canvas.height;
		const mid = H * 0.5;
		ctx.clearRect(0, 0, W, H);
		ctx.fillStyle = WAVE_FILL;

		for (let px = 0; px < W; px++) {
			const g = _grow[px];
			if (g <= 0.001) continue;
			const yTop = mid + (_showTops[px] - mid) * g;
			const yBot = mid + (_showBots[px] - mid) * g;
			ctx.fillRect(px, yTop, 1, Math.max(1, yBot - yTop));
		}
	}

	function signalPendingPainted() {
		if (paintSignaled || !pendingImportId) return;
		paintSignaled = true;
		dispatch('pendingpainted', { pendingImportId });
	}

	function paintFinalBitmap(bitmap: ImageBitmap) {
		if (!canvas) return;
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		// Setting canvas.width clears pixels — only do it when size actually changes.
		if (canvas.width !== bitmap.width) canvas.width = bitmap.width;
		ctx.clearRect(0, 0, canvas.width, canvas.height);
		ctx.drawImage(bitmap, 0, 0);
		_drawnBitmap = bitmap;
	}

	function tickGrow(now = performance.now()) {
		_raf = 0;
		if (!canvas || !pending) return;

		const W = Math.max(1, canvasCssW);
		const H = Math.max(canvas.height, Math.max(clipEl?.clientHeight ?? 0, 96));
		const freezeGoals =
			(loadProgress >= 0.93 || holdWorkerAfterHandoff) &&
			(peaks_512.length > 0 || peaks_4096.length > 0 || peaks_64.length > 0) &&
			_goalTops.length > 0;
		// Resizing the canvas buffer clears pixels — avoid that during handoff.
		if (!freezeGoals) {
			if (canvas.width !== W) canvas.width = W;
			if (canvas.height !== H) canvas.height = H;
			const sig = `${peaksEpoch}|${W}|${H}|${scaleBucket(scale)}|${offset.toFixed(4)}`;
			if (_colsDirty || sig !== _peaksSig || _goalTops.length !== W) {
				_peaksSig = sig;
				rebuildGoals(W, H);
			}
		} else if (_goalTops.length === 0) {
			if (canvas.width !== W) canvas.width = W;
			if (canvas.height !== H) canvas.height = H;
			rebuildGoals(W, H, true);
		}

		const covered = peaksCoveragePx(W);
		const dt = _lastTs > 0 ? Math.min(40, Math.max(8, now - _lastTs)) / 16.67 : 1;
		_lastTs = now;

		const dCovered = covered - _rawCovered;
		if (dCovered > 0.5) {
			// Rate from time between peak batches only — ignores empty frames.
			if (_lastBatchAt > 0) {
				const elapsedFrames = Math.max(1, (now - _lastBatchAt) / 16.67);
				const batchRate = dCovered / elapsedFrames;
				const capped = Math.min(batchRate, Math.max(3, W * 0.12));
				_loadRate = _loadRate <= 0.01 ? capped * 0.55 : _loadRate * 0.7 + capped * 0.3;
			} else {
				_loadRate = Math.min(Math.max(1.2, dCovered * 0.2), W * 0.05);
			}
			_lastBatchAt = now;
		}
		_rawCovered = covered;

		const finishing = loadProgress >= 0.96 || covered >= W - 1;
		const lag = finishing ? 0 : Math.max(10, W * 0.09);
		const floorLag = finishing ? 0 : Math.max(3, W * 0.02);
		const ceiling = Math.max(0, covered - floorLag);

		// Desired coast from batch-rate + slack; then heavily smooth speed so batch edges don't jerk.
		const slack = covered - _smoothCovered;
		const steer = finishing ? 1.4 : 0.7 + 0.55 * (slack / Math.max(lag, 1));
		let desiredCoast = _loadRate * Math.min(1.4, Math.max(0.35, steer));
		const room = ceiling - _smoothCovered;
		if (!finishing && room < lag) {
			desiredCoast *= Math.max(0.12, room / Math.max(lag, 1));
		}
		_coastSpeed += (desiredCoast - _coastSpeed) * (1 - Math.pow(0.86, dt));

		if (room > 0.05) {
			_smoothCovered = Math.min(ceiling, _smoothCovered + Math.max(0, _coastSpeed) * dt);
		}

		_head = _smoothCovered;

		// Morph + grow only the visible prefix — avoid O(W) work far ahead of the head.
		const morph = 1 - Math.pow(0.82, dt);
		const settlePx = 10;
		let stillGrowing = false;
		const limit = Math.min(W, Math.ceil(_head));
		const morphUntil = Math.min(W, limit + 2);
		for (let px = 0; px < morphUntil; px++) {
			_showTops[px] += (_goalTops[px] - _showTops[px]) * morph;
			_showBots[px] += (_goalBots[px] - _showBots[px]) * morph;
			if (px < limit) {
				const age = _head - px;
				const linear = age >= settlePx ? 1 : Math.max(0, age / settlePx);
				const eased = 1 - (1 - linear) ** 3;
				if (eased > _grow[px]) _grow[px] = eased;
				if (_grow[px] < 0.995) stillGrowing = true;
			}
		}

		const ctx = canvas.getContext('2d');
		if (ctx) paintGrowingColumns(ctx);

		const catchingUp = _smoothCovered < covered - 0.5 || stillGrowing || !finishing;
		if (pending && catchingUp) {
			_raf = requestAnimationFrame(tickGrow);
		}
	}

	function ensureGrowLoop() {
		if (!pending) return;
		if (!_raf) _raf = requestAnimationFrame(tickGrow);
	}

	$: if (pending) {
		void peaksEpoch;
		void canvasCssW;
		void loadProgress;
		// Freeze peak-lane rebuilds near the end — Rust finalize replaces 512/4096
		// with recomputed lanes that differ slightly from streamed deltas (end flash).
		// Exception: still allow the first real peak fill (cached re-import jumps to
		// progress=1 before peaks land).
		const hasPeaks = peaks_64.length > 0 || peaks_512.length > 0 || peaks_4096.length > 0;
		if (loadProgress < 0.93 || !hasPeaks) {
			_colsDirty = true;
		}
		ensureGrowLoop();
	}

	const loadWorker = async () => {
		const SyncWorker = await import('$lib/waveform.worker?worker');
		syncWorker = new SyncWorker.default();
		syncWorker.onmessage = ({ data }: MessageEvent<{ bitmap: ImageBitmap; key: string }>) => {
			bitmapCache.set(data.key, data.bitmap);
			if (pending) return;
			if (data.key === latestKey) paintFinalBitmap(data.bitmap);
		};
		workerReady = true;
	};

	function requestRender(targetWidth: number) {
		if (pending || !syncWorker || !canvas.height) return;
		if (holdWorkerAfterHandoff) {
			holdWorkerAfterHandoff = false;
		}
		const bucket = scaleBucket(scale);
		const key = renderKey(bucket, offset, peaksEpoch);
		latestKey = key;
		const cached = bitmapCache.get(key);
		if (cached && cached.width === targetWidth) {
			if (_drawnBitmap !== cached) paintFinalBitmap(cached);
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
		const h = Math.max(clipEl.clientHeight, 96);
		canvas.width = canvasCssW;
		canvas.height = h;
		loadWorker();
		if (pending) {
			_head = 0;
			_loadRate = 0;
			_coastSpeed = 0;
			_smoothCovered = 0;
			_rawCovered = 0;
			_lastBatchAt = 0;
			_lastTs = 0;
			ensureGrowLoop();
		}
	});

	onDestroy(() => {
		if (_raf) cancelAnimationFrame(_raf);
	});

	function onContextMenu(e: MouseEvent) {
		e.preventDefault();
		e.stopPropagation();
		selection.select(id, { additive: e.ctrlKey || e.metaKey });
		if (pending) return;
		dispatch('clipcontextmenu', { id, x: e.clientX, y: e.clientY });
	}

	function onKeyDown(e: KeyboardEvent) {
		if (e.key !== 'Enter' && e.key !== ' ') return;
		e.preventDefault();
		e.stopPropagation();
	}

	// Handoff: set hold first. Do not read/write canvas here (cyclical deps with
	// the height/worker reactives below).
	$: if (wasPending && !pending) {
		wasPending = false;
		if (_raf) {
			cancelAnimationFrame(_raf);
			_raf = 0;
		}
		// Cached re-import settles in ms — grow never covers the frame. Holding
		// that empty canvas leaves a flat waveform; let the worker paint instead.
		const grewEnough = _smoothCovered >= Math.max(8, canvasCssW * 0.85);
		// Grow paints streamed lanes only (no peaks_64) — if the dense lane is
		// already here, skip hold so zoom stays sharp via the worker.
		holdWorkerAfterHandoff = grewEnough && peaks_64.length === 0;
		holdScaleBucket = scaleBucket(scale);
		holdWidth = canvasCssW;
		holdPeaksEpoch = peaksEpoch;
		importMark('clip-pending-false');
		signalPendingPainted();
	}
	$: if (pending) {
		wasPending = true;
		paintSignaled = false;
		holdWorkerAfterHandoff = false;
	}

	// Release hold when zoom/width/peaks change — otherwise CSS stretches the
	// grow bitmap (width:100%) and the waveform looks blurry.
	$: if (holdWorkerAfterHandoff) {
		void canvasCssW;
		void peaksEpoch;
		void scale;
		if (
			scaleBucket(scale) !== holdScaleBucket ||
			canvasCssW !== holdWidth ||
			peaksEpoch !== holdPeaksEpoch
		) {
			holdWorkerAfterHandoff = false;
		}
	}

	$: if (canvas && clipEl && !pending && !holdWorkerAfterHandoff) {
		const h = Math.max(clipEl.clientHeight, 96);
		if (h > canvas.height) {
			canvas.height = h;
			requestRender(canvasCssW);
		}
	}

	$: if (canvas && workerReady && !pending && !holdWorkerAfterHandoff) {
		const w = canvasCssW;
		const key = renderKey(scaleBucket(scale), offset, peaksEpoch);
		if (needsRender(w, key)) requestRender(w);
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
		if (pending) return;
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
	class:pending
	role="button"
	tabindex="0"
	aria-busy={pending}
	style="transform: translateX({frame.leftPx}px); width: {frame.widthPx}px"
	on:mousedown={onMouseDown}
	on:contextmenu={onContextMenu}
	on:click|stopPropagation
	on:keydown={onKeyDown}
>
	<canvas bind:this={canvas}></canvas>
	<!-- Always mounted — toggling these on pending→false caused a layout pop. -->
	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div
		class="resize-handle resize-handle--left"
		class:resize-handle--disabled={pending}
		on:mousedown={(e) => onResizeHandleDown('left', e)}
	></div>
	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div
		class="resize-handle resize-handle--right"
		class:resize-handle--disabled={pending}
		on:mousedown={(e) => onResizeHandleDown('right', e)}
	></div>
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
			box-shadow: inset 0 0 0 1px rgba(120, 180, 255, 0.85);
		}

		&.pending {
			/* Same chrome as a real clip — only waveform coverage differs. */
			cursor: grab;

			&:active {
				cursor: grabbing;
			}
		}

		&.pending.selected {
			background-color: #1a4fa0;
		}

		canvas {
			display: block;
			width: 100%;
			height: 100%;
			background-color: transparent;
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

			&--disabled {
				pointer-events: none;
				cursor: inherit;
			}
		}
	}
</style>
