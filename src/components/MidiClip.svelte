<!-- MidiClip.svelte — MIDI clip in the arrange view -->
<script lang="ts">
	import { onMount, onDestroy, createEventDispatcher } from 'svelte';
	import type { MidiNote } from '../types/global';
	import { selection } from '../stores/selection';
	import { arrangeClipFrame } from '../frontend/arrangeInteraction';

	export let id: string;
	export let start: number;
	export let end: number;
	export let offset: number = 0;
	export let scale: number;
	export let notes: MidiNote[];
	/** Another clip plays the same content (ADR-032). */
	export let linked = false;
	export let color: string = '#6c63ff';
	export let bpm: number = 120;
	/** Shared pitch bounds for all MIDI clips on the same lane. */
	export let minPitch: number | undefined = undefined;
	export let maxPitch: number | undefined = undefined;
	/** Live preview of a running recording: same look, not editable. */
	export let recording = false;

	const dispatch = createEventDispatcher<{
		midiclip_dragstart: { offsetPx: number; duplicate: boolean; linked: boolean };
		midiclip_resizestart: { edge: 'left' | 'right'; clientX: number };
		midiclip_repeatstart: { clientX: number };
		midiclip_contextmenu: { id: string; x: number; y: number };
		midiclip_dblclick: { id: string };
		midiclip_activate: { id: string };
	}>();

	const CANVAS_INSET = { top: 14, right: 2, bottom: 4, left: 2 };
	const dpr = typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1;

	let clipEl: HTMLElement;
	let canvas: HTMLCanvasElement;
	let rafId = 0;
	let resizeObserver: ResizeObserver | undefined;

	$: selected = $selection.ids.has(id);
	$: frame = arrangeClipFrame(start, end, scale, dpr);

	// Re-render whenever notes, frame, pitch range, bpm, color, or trim offset change.
	$: if (canvas)
		scheduleRender(notes, frame.widthPx, bpm, color, selected, offset, minPitch, maxPitch);

	function scheduleRender(..._deps: unknown[]) {
		cancelAnimationFrame(rafId);
		rafId = requestAnimationFrame(renderPreview);
	}

	function drawSize(): { cssW: number; cssH: number; timeW: number } {
		const timeW = Math.max(1, frame.widthPx);
		const cssW = Math.max(1, timeW - CANVAS_INSET.left - CANVAS_INSET.right);
		const cssH = Math.max(1, (clipEl?.clientHeight ?? 0) - CANVAS_INSET.top - CANVAS_INSET.bottom);
		return { cssW, cssH, timeW };
	}

	function syncCanvasSize(cssW: number, cssH: number) {
		if (!canvas) return;
		const w = Math.max(1, Math.round(cssW * dpr));
		const h = Math.max(1, Math.round(cssH * dpr));
		if (canvas.width !== w) canvas.width = w;
		if (canvas.height !== h) canvas.height = h;
		canvas.style.width = `${cssW}px`;
		canvas.style.height = `${cssH}px`;
	}

	function renderPreview() {
		if (!canvas || !clipEl) return;
		const { cssW, cssH, timeW } = drawSize();
		syncCanvasSize(cssW, cssH);

		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, cssW, cssH);

		if (notes.length === 0) return;

		const pitches = notes.map((n) => n.pitch);
		const minP = minPitch ?? Math.max(0, Math.min(...pitches) - 3);
		const maxP = maxPitch ?? Math.min(127, Math.max(...pitches) + 3);
		const pitchRange = maxP - minP || 1;

		const clipDurationBeats = ((end - start) * bpm) / 60;
		const beatRange = clipDurationBeats || 1;
		const offsetBeats = (offset * bpm) / 60;

		const r = parseInt(color.slice(1, 3), 16);
		const g = parseInt(color.slice(3, 5), 16);
		const b = parseInt(color.slice(5, 7), 16);

		for (const note of notes) {
			// Map against full clip width (not inset) so positions stay stable while resizing.
			const xFull = ((note.start - offsetBeats) / beatRange) * timeW;
			const rawW = Math.max(1.5, (note.duration / beatRange) * timeW - 0.5);
			const x = xFull - CANVAS_INSET.left;
			if (x >= cssW || x + rawW <= 0) continue;

			const xDraw = Math.max(0, x);
			const wDraw = Math.min(cssW - xDraw, rawW - (xDraw - x));
			if (wDraw <= 0) continue;

			const y = ((maxP - note.pitch) / pitchRange) * (cssH - 2) + 1;
			const h = Math.max(1.5, (cssH - 2) / pitchRange);

			const alpha = selected ? 1 : 0.55 + (note.velocity / 127) * 0.45;
			ctx.fillStyle = `rgba(${r},${g},${b},${alpha})`;
			ctx.beginPath();
			ctx.roundRect(xDraw, y, wDraw, h, 1);
			ctx.fill();
		}
	}

	function onKeyDown(e: KeyboardEvent) {
		if (e.key !== 'Enter' && e.key !== ' ') return;
		e.preventDefault();
		e.stopPropagation();
	}

	function onMouseDown(e: MouseEvent) {
		e.stopPropagation();
		e.preventDefault();

		selection.select(id, { additive: e.ctrlKey || e.metaKey });
		dispatch('midiclip_activate', { id });

		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		dispatch('midiclip_dragstart', {
			offsetPx: e.clientX - rect.left,
			duplicate: e.altKey,
			linked: e.altKey && e.shiftKey
		});
	}

	function onResizeHandleDown(edge: 'left' | 'right', e: MouseEvent) {
		e.stopPropagation();
		e.preventDefault();
		selection.select(id, { additive: e.ctrlKey || e.metaKey });
		dispatch('midiclip_activate', { id });
		dispatch('midiclip_resizestart', { edge, clientX: e.clientX });
	}

	function onRepeatHandleDown(e: MouseEvent) {
		e.stopPropagation();
		e.preventDefault();
		selection.select(id, { additive: e.ctrlKey || e.metaKey });
		dispatch('midiclip_activate', { id });
		dispatch('midiclip_repeatstart', { clientX: e.clientX });
	}

	function onDblClick(e: MouseEvent) {
		e.stopPropagation();
		e.preventDefault();
		dispatch('midiclip_dblclick', { id });
	}

	function onContextMenu(e: MouseEvent) {
		e.preventDefault();
		e.stopPropagation();
		dispatch('midiclip_contextmenu', { id, x: e.clientX, y: e.clientY });
	}

	onMount(() => {
		renderPreview();
		resizeObserver = new ResizeObserver(() => scheduleRender());
		resizeObserver.observe(clipEl);
	});

	onDestroy(() => {
		cancelAnimationFrame(rafId);
		resizeObserver?.disconnect();
	});
</script>

<div
	bind:this={clipEl}
	class="midi-clip"
	class:selected
	class:recording
	role="button"
	tabindex="0"
	style="transform: translateX({frame.leftPx}px); width: {frame.widthPx}px; --clip-color: {color};"
	on:mousedown={onMouseDown}
	on:dblclick={onDblClick}
	on:contextmenu={onContextMenu}
	on:click|stopPropagation
	on:keydown={onKeyDown}
>
	<div class="midi-clip__header">
		<svg class="midi-clip__icon" viewBox="0 0 10 10" width="8" height="8" fill="currentColor">
			<rect x="0" y="2" width="1.5" height="6" rx="0.5" />
			<rect x="2.5" y="0" width="1.5" height="8" rx="0.5" />
			<rect x="5" y="3" width="1.5" height="5" rx="0.5" />
			<rect x="7.5" y="1" width="1.5" height="7" rx="0.5" />
		</svg>
		{#if linked}
			<svg
				class="midi-clip__icon"
				viewBox="0 0 24 24"
				width="9"
				height="9"
				fill="none"
				stroke="currentColor"
				stroke-width="3"
				stroke-linecap="round"
				stroke-linejoin="round"
				aria-hidden="true"
			>
				<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
				<path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
			</svg>
		{/if}
	</div>
	<canvas bind:this={canvas} class="midi-clip__canvas"></canvas>
	{#if !recording}
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<div
			class="resize-handle resize-handle--left"
			on:mousedown={(e) => onResizeHandleDown('left', e)}
		></div>
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<div
			class="resize-handle resize-handle--right"
			on:mousedown={(e) => onResizeHandleDown('right', e)}
		></div>
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<div class="repeat-handle" title="Glisser pour répéter" on:mousedown={onRepeatHandleDown}></div>
	{/if}
</div>

<style lang="scss">
	.midi-clip {
		position: absolute;
		top: 0;
		bottom: 0;
		overflow: hidden;
		border-radius: 4px;
		cursor: grab;
		background-color: color-mix(in srgb, var(--clip-color) 18%, #1a1a2e);
		box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--clip-color) 60%, transparent);

		&:active {
			cursor: grabbing;
		}

		&.recording {
			pointer-events: none;
		}

		&.selected {
			background-color: color-mix(in srgb, var(--clip-color) 30%, #1a1a2e);
			box-shadow: inset 0 0 0 1.5px var(--clip-color);
		}

		&__header {
			position: absolute;
			top: 2px;
			left: 4px;
			color: color-mix(in srgb, var(--clip-color) 90%, white);
			display: flex;
			align-items: center;
			gap: 3px;
			pointer-events: none;
			opacity: 0.7;
		}

		&__icon {
			flex-shrink: 0;
		}

		&__canvas {
			position: absolute;
			top: 14px;
			left: 2px;
			display: block;
			pointer-events: none;
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

		.repeat-handle {
			position: absolute;
			top: 0;
			right: 0;
			width: 10px;
			height: 10px;
			cursor: e-resize;
			z-index: 3;
			border-top-right-radius: 4px;

			&::after {
				content: '';
				position: absolute;
				top: 2px;
				right: 2px;
				width: 4px;
				height: 4px;
				border-radius: 50%;
				background: color-mix(in srgb, var(--clip-color) 90%, white);
				opacity: 0;
			}

			&:hover::after {
				opacity: 0.8;
			}
		}
	}
</style>
