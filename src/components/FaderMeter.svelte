<!-- FaderMeter.svelte
     Horizontal gain fader with dual-channel (L/R) post-fader level meters.
     Two stacked bars show L and R independently; the thumb spans both and
     sets the gain.  Software decay keeps display smooth between 33 ms ticks.
-->
<script lang="ts">
	import { themeVar } from '../frontend/theme';
	import { onMount, onDestroy, createEventDispatcher } from 'svelte';
	import { modulationDrag } from '../stores/modulationDrag';
	import { automationRequest } from '../stores/automation';

	/** Gain value, 0–100. */
	export let value: number = 75;
	/** Post-fader peak, left channel, linear 0–1. */
	export let peakL: number = 0;
	/** Post-fader peak, right channel, linear 0–1. */
	export let peakR: number = 0;
	/** Value snapped to on double-click. */
	export let defaultValue: number = 75;
	/** Color of the modulator driving this fader; draws a dashed outline when set. */
	export let modColor: string | undefined = undefined;
	/** Modulation target key; makes this fader a drop target for dragged modulators. */
	export let modTargetKey: string | undefined = undefined;
	/** Track color to draw as a solid ring when a lane with points drives the volume (ADR-025). */
	export let automationColor: string | undefined = undefined;
	/** Value the automation lane asks for at the playhead (0-100); shown instead of `value` unless dragging. */
	export let automatedValue: number | undefined = undefined;

	$: dropCandidate = modTargetKey !== undefined && $modulationDrag !== null;
	$: dropHover = dropCandidate && $modulationDrag?.hoverKey === modTargetKey;

	const dispatch = createEventDispatcher<{ change: number; release: number }>();

	// ── dBFS helpers ─────────────────────────────────────────────────────────────

	const DB_FLOOR = -60;

	function linearToDb(v: number): number {
		return v <= 0 ? DB_FLOOR : Math.max(DB_FLOOR, 20 * Math.log10(v));
	}

	function dbToPercent(db: number): number {
		return Math.max(0, Math.min(100, ((db - DB_FLOOR) / -DB_FLOOR) * 100));
	}

	function meterColor(pct: number): string {
		if (pct >= 95) return themeVar('meter-high');
		if (pct >= 75) return themeVar('meter-mid');
		return themeVar('meter-low');
	}

	// ── Per-channel soft decay (60 fps) ──────────────────────────────────────────

	let dispL = 0;
	let dispR = 0;
	let rafId: number;

	function tick() {
		dispL = peakL > dispL ? peakL : Math.max(0, dispL * 0.985);
		dispR = peakR > dispR ? peakR : Math.max(0, dispR * 0.985);
		rafId = requestAnimationFrame(tick);
	}

	onMount(() => {
		rafId = requestAnimationFrame(tick);
	});
	onDestroy(() => cancelAnimationFrame(rafId));

	// ── Derived display values ────────────────────────────────────────────────────

	$: pctL = dbToPercent(linearToDb(dispL));
	$: pctR = dbToPercent(linearToDb(dispR));
	$: colorL = meterColor(pctL);
	$: colorR = meterColor(pctR);
	$: faderPct = Math.max(
		0,
		Math.min(100, automatedValue !== undefined && !dragging ? automatedValue : value)
	);

	// ── Drag interaction ──────────────────────────────────────────────────────────

	let rail: HTMLElement;
	let dragging = false;

	function clampedVal(clientX: number): number {
		if (!rail) return value;
		const r = rail.getBoundingClientRect();
		return Math.max(0, Math.min(100, ((clientX - r.left) / r.width) * 100));
	}

	function onMouseDown(e: MouseEvent) {
		e.preventDefault();
		dragging = true;
		value = clampedVal(e.clientX);
		dispatch('change', value);
		window.addEventListener('mousemove', onMouseMove);
		window.addEventListener('mouseup', onMouseUp);
	}

	function onMouseMove(e: MouseEvent) {
		if (!dragging) return;
		value = clampedVal(e.clientX);
		dispatch('change', value);
	}

	function onMouseUp(e: MouseEvent) {
		dragging = false;
		value = clampedVal(e.clientX);
		dispatch('release', value);
		window.removeEventListener('mousemove', onMouseMove);
		window.removeEventListener('mouseup', onMouseUp);
	}

	function onContextMenu(e: MouseEvent) {
		if (modTargetKey === undefined) return;
		e.preventDefault();
		e.stopPropagation();
		automationRequest.set({ key: modTargetKey, x: e.clientX, y: e.clientY });
	}

	function onDblClick() {
		value = defaultValue;
		dispatch('change', value);
		dispatch('release', value);
	}
</script>

<div
	class="fader-meter"
	class:fader-meter--dragging={dragging}
	class:drop-candidate={dropCandidate}
	class:drop-hover={dropHover}
	style:--drop-color={$modulationDrag?.color}
	data-mod-target={modTargetKey}
	bind:this={rail}
	on:mousedown={onMouseDown}
	on:dblclick={onDblClick}
	on:contextmenu={onContextMenu}
	role="slider"
	aria-valuemin={0}
	aria-valuemax={100}
	aria-valuenow={Math.round(value)}
	tabindex="0"
>
	<!-- L channel bar -->
	<div class="channel">
		<div class="fill" style="width:{pctL}%; background:{colorL};"></div>
	</div>

	<!-- R channel bar -->
	<div class="channel">
		<div class="fill" style="width:{pctR}%; background:{colorR};"></div>
	</div>

	{#if automationColor}
		<div class="automation-ring" style="border-color:{automationColor};"></div>
	{/if}
	{#if modColor}
		<div class="mod-ring" style="border-color:{modColor};"></div>
	{/if}

	<!-- Gain thumb — spans both channels -->
	<div class="thumb" style="left:{faderPct}%;"></div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.fader-meter {
		position: relative;
		width: 100%;
		display: flex;
		flex-direction: column;
		gap: 2px;
		cursor: ew-resize;
		user-select: none;
		outline: none;
		padding: 1px 0;

		&.drop-candidate {
			border-radius: 3px;
			box-shadow: 0 0 0 2px color-mix(in srgb, var(--drop-color) 45%, transparent);
		}

		&.drop-hover {
			box-shadow: 0 0 0 3px var(--drop-color);
			background: color-mix(in srgb, var(--drop-color) 20%, transparent);
		}

		&:focus-visible .channel {
			outline: 1px solid $accent-border;
			outline-offset: 1px;
		}
	}

	.channel {
		height: 5px;
		border-radius: 2px;
		background: $surface-3;
		position: relative;
		overflow: hidden;
	}

	.fill {
		position: absolute;
		inset: 0 auto 0 0;
		border-radius: 2px;
		opacity: 0.7;
		pointer-events: none;
		will-change: width;
	}

	.mod-ring {
		position: absolute;
		inset: -2px -3px;
		border: 1px dashed;
		border-radius: 3px;
		opacity: 0.65;
		pointer-events: none;
	}

	.automation-ring {
		position: absolute;
		inset: -4px -5px;
		border: 1.5px solid;
		border-radius: 4px;
		opacity: 0.9;
		pointer-events: none;
	}

	.thumb {
		position: absolute;
		top: 50%;
		transform: translate(-50%, -50%);
		width: 3px;
		height: 20px;
		background: $text-strong;
		border-radius: 2px;
		pointer-events: none;
		box-shadow: $shadow-sm;
		will-change: left;
	}
</style>
