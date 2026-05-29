<!-- MidiClip.svelte — MIDI clip in the arrange view -->
<script lang="ts">
	import { onMount, onDestroy, createEventDispatcher } from 'svelte';
	import type { MidiNote } from '../types/global';
	import { selection } from '../stores/selection';
	import { keyboardFocus } from '../stores/keyboardFocus';

	export let id: string;
	export let start: number;
	export let end: number;
	export let scale: number;
	export let notes: MidiNote[];
	export let color: string = '#6c63ff';
	export let bpm: number = 120;

	const dispatch = createEventDispatcher<{
		midiclip_dragstart: { offsetPx: number };
		midiclip_contextmenu: { id: string; x: number; y: number };
		midiclip_dblclick: { id: string };
	}>();

	let clipEl: HTMLElement;
	let canvas: HTMLCanvasElement;
	let rafId = 0;

	$: selected = $selection.has(id);
	$: width = Math.max(2, (end - start) * scale);

	// Re-render whenever notes, width, bpm, or color change.
	$: if (canvas) scheduleRender(notes, width, bpm, color, selected);

	function scheduleRender(..._deps: unknown[]) {
		cancelAnimationFrame(rafId);
		rafId = requestAnimationFrame(renderPreview);
	}

	function renderPreview() {
		if (!canvas) return;
		const W = Math.max(1, Math.ceil(width));
		const H = canvas.height;
		if (canvas.width !== W) canvas.width = W;

		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		ctx.clearRect(0, 0, W, H);

		if (notes.length === 0) return;

		// Determine pitch range with a margin so single notes aren't full height
		const pitches = notes.map((n) => n.pitch);
		const minP = Math.max(0, Math.min(...pitches) - 3);
		const maxP = Math.min(127, Math.max(...pitches) + 3);
		const pitchRange = maxP - minP || 1;

		// Use the actual clip duration in beats so note positions are correct
		// regardless of how many notes are present or where they sit.
		const clipDurationBeats = (end - start) * bpm / 60;
		const beatRange = clipDurationBeats || 1;

		const r = parseInt(color.slice(1, 3), 16);
		const g = parseInt(color.slice(3, 5), 16);
		const b = parseInt(color.slice(5, 7), 16);

		for (const note of notes) {
			const x = (note.start / beatRange) * W;
			const w = Math.max(1.5, (note.duration / beatRange) * W - 0.5);
			const y = ((maxP - note.pitch) / pitchRange) * (H - 2) + 1;
			const h = Math.max(1.5, (H - 2) / pitchRange);

			const alpha = selected ? 1 : 0.55 + (note.velocity / 127) * 0.45;
			ctx.fillStyle = `rgba(${r},${g},${b},${alpha})`;
			ctx.beginPath();
			ctx.roundRect(x, y, w, h, 1);
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

		keyboardFocus.setTimeline();

		selection.update((sel) => {
			const next = new Set(sel);
			if (e.ctrlKey || e.metaKey) {
				if (next.has(id)) next.delete(id);
				else next.add(id);
			} else if (!next.has(id)) {
				next.clear();
				next.add(id);
			}
			return next;
		});

		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		dispatch('midiclip_dragstart', { offsetPx: e.clientX - rect.left });
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
		canvas.width = Math.max(1, Math.ceil(width));
		canvas.height = clipEl.clientHeight;
		renderPreview();
	});

	onDestroy(() => cancelAnimationFrame(rafId));
</script>

<div
	bind:this={clipEl}
	class="midi-clip"
	class:selected
	role="button"
	tabindex="0"
	style="transform: translateX({start * scale}px); width: {width}px; --clip-color: {color};"
	on:mousedown={onMouseDown}
	on:dblclick={onDblClick}
	on:contextmenu={onContextMenu}
	on:click|stopPropagation
	on:keydown={onKeyDown}
>
	<div class="midi-clip__header">
		<svg class="midi-clip__icon" viewBox="0 0 10 10" width="8" height="8" fill="currentColor">
			<rect x="0" y="2" width="1.5" height="6" rx="0.5"/>
			<rect x="2.5" y="0" width="1.5" height="8" rx="0.5"/>
			<rect x="5" y="3" width="1.5" height="5" rx="0.5"/>
			<rect x="7.5" y="1" width="1.5" height="7" rx="0.5"/>
		</svg>
	</div>
	<canvas bind:this={canvas} class="midi-clip__canvas"></canvas>
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

		&:active { cursor: grabbing; }

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
			inset: 14px 2px 4px;
			display: block;
		}
	}
</style>
