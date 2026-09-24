<!-- LfoCard.svelte — one LFO: live waveform display, shape, rate / tempo sync, drag-to-assign handle -->
<script context="module" lang="ts">
	import { transport } from '../stores/transport';

	// Shared by every card: the engine resets LFO phase when playback starts.
	let playStartMs: number | null = null;
	transport.subscribe((t) => {
		if (t.isPlaying && playStartMs === null) playStartMs = performance.now();
		else if (!t.isPlaying) playStartMs = null;
	});
</script>

<script lang="ts">
	import { createEventDispatcher, onDestroy } from 'svelte';
	import Knob from './Knob.svelte';
	import { tempo } from '../stores/tempo';
	import { modulationDrag } from '../stores/modulationDrag';
	import { updateModulator } from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';
	import type { LfoWaveform, ProjectModulator } from '../types/global';

	export let trackId: string;
	export let mod: ProjectModulator;
	export let label: string;
	export let color: string;
	export let pickerOpen = false;

	const dispatch = createEventDispatcher<{ assign: string; togglepicker: void; remove: void }>();

	const SHAPES: { id: LfoWaveform; label: string; icon: string }[] = [
		{ id: 'sine', label: 'Sinus', icon: 'M0 4 C2.5 -1.5 4.5 -1.5 7 4 C9.5 9.5 11.5 9.5 14 4' },
		{ id: 'triangle', label: 'Triangle', icon: 'M0 4 L3.5 0.5 L10.5 7.5 L14 4' },
		{ id: 'saw', label: 'Dent de scie', icon: 'M0 7.5 L13 0.5 L13 7.5' },
		{ id: 'square', label: 'Carré', icon: 'M0 4 L0 0.5 L7 0.5 L7 7.5 L14 7.5 L14 4' },
	];

	// Beats per cycle, labelled as note values (a beat is a quarter note).
	const DIVISIONS = [
		{ beats: 0.25, label: '1/16' },
		{ beats: 0.5, label: '1/8' },
		{ beats: 1, label: '1/4' },
		{ beats: 2, label: '1/2' },
		{ beats: 4, label: '1/1' },
		{ beats: 8, label: '2/1' },
		{ beats: 16, label: '4/1' },
	];

	const RATE_MIN_HZ = 0.1;
	const RATE_MAX_HZ = 20;

	function rateToKnob(rateHz: number): number {
		return Math.log(rateHz / RATE_MIN_HZ) / Math.log(RATE_MAX_HZ / RATE_MIN_HZ);
	}

	function knobToRate(position: number): number {
		return RATE_MIN_HZ * (RATE_MAX_HZ / RATE_MIN_HZ) ** position;
	}

	function divisionIndex(beats: number): number {
		let best = 0;
		DIVISIONS.forEach((d, i) => {
			if (Math.abs(d.beats - beats) < Math.abs(DIVISIONS[best].beats - beats)) best = i;
		});
		return best;
	}

	function formatRate(rateHz: number): string {
		return `${rateHz < 10 ? rateHz.toFixed(2) : rateHz.toFixed(1)} Hz`;
	}

	function sample(phase: number, shape: LfoWaveform): number {
		switch (shape) {
			case 'sine': return Math.sin(phase * 2 * Math.PI);
			case 'triangle': return phase < 0.25 ? 4 * phase : phase < 0.75 ? 2 - 4 * phase : 4 * phase - 4;
			case 'saw': return 2 * phase - 1;
			case 'square': return phase < 0.5 ? 1 : -1;
		}
	}

	// Display geometry, in viewBox units (100 × 32).
	const MID_Y = 16;
	const AMPLITUDE = 13;
	const CURVE_POINTS = 200;

	function waveLine(shape: LfoWaveform): string {
		const points: string[] = [];
		for (let i = 0; i <= CURVE_POINTS; i++) {
			const phase = Math.min(i / CURVE_POINTS, 0.99999);
			points.push(`${(i / CURVE_POINTS) * 100},${MID_Y - sample(phase, shape) * AMPLITUDE}`);
		}
		return `M${points.join(' L')}`;
	}

	$: shape = mod.waveform ?? 'sine';
	$: sync = mod.tempo_sync ?? false;
	$: line = waveLine(shape);
	$: area = `${line} L100,${MID_Y} L0,${MID_Y} Z`;

	let knobValue = 0;
	$: knobValue = sync ? divisionIndex(mod.beat_division ?? 1) : rateToKnob(mod.rate_hz ?? 1);
	$: liveDivision = DIVISIONS[Math.round(knobValue)] ?? DIVISIONS[2];
	$: liveRateHz = sync ? $tempo.bpm / (60 * liveDivision.beats) : knobToRate(knobValue);
	$: rateLabel = sync ? liveDivision.label : formatRate(liveRateHz);

	async function update(changes: Parameters<typeof updateModulator>[2]) {
		try { await updateModulator(trackId, mod.id, changes); }
		catch (e) { notify(`Erreur LFO : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	function handleKnobRelease(position: number) {
		if (sync) update({ beat_division: DIVISIONS[Math.round(position)].beats });
		else update({ rate_hz: Number(knobToRate(position).toFixed(3)) });
	}

	// ── Playhead: mirrors the engine phase while the transport runs ───────────
	let phase = 0;
	let playing = false;
	let frameId = 0;
	let lastFrameMs = 0;

	$: if ($transport.isPlaying) startPlayhead(liveRateHz);
	else stopPlayhead();

	function startPlayhead(rateHz: number) {
		if (frameId) return;
		const now = performance.now();
		phase = (((now - (playStartMs ?? now)) / 1000) * rateHz) % 1;
		lastFrameMs = now;
		playing = true;
		frameId = requestAnimationFrame(advancePlayhead);
	}

	function advancePlayhead(now: number) {
		phase = (phase + ((now - lastFrameMs) / 1000) * liveRateHz) % 1;
		lastFrameMs = now;
		frameId = requestAnimationFrame(advancePlayhead);
	}

	function stopPlayhead() {
		cancelAnimationFrame(frameId);
		frameId = 0;
		playing = false;
		phase = 0;
	}

	$: dotTopPercent = ((MID_Y - sample(phase, shape) * AMPLITUDE) / 32) * 100;

	// ── Drag the handle onto a knob to assign ─────────────────────────────────
	const DRAG_THRESHOLD_PX = 4;

	let dragStart: { x: number; y: number } | null = null;
	let ghost: { x: number; y: number } | null = null;

	function targetKeyAt(x: number, y: number): string | null {
		const el = document.elementFromPoint(x, y)?.closest<HTMLElement>('[data-mod-target]');
		return el?.dataset.modTarget ?? null;
	}

	function handleDragStart(e: PointerEvent) {
		if (e.button !== 0) return;
		e.preventDefault();
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		dragStart = { x: e.clientX, y: e.clientY };
	}

	function handleDragMove(e: PointerEvent) {
		if (!dragStart) return;
		if (!ghost) {
			if (Math.hypot(e.clientX - dragStart.x, e.clientY - dragStart.y) < DRAG_THRESHOLD_PX) return;
			if (pickerOpen) dispatch('togglepicker');
			document.body.style.cursor = 'grabbing';
			modulationDrag.set({ modulatorId: mod.id, color, hoverKey: null });
		}
		ghost = { x: e.clientX, y: e.clientY };
		const hoverKey = targetKeyAt(e.clientX, e.clientY);
		modulationDrag.update((d) => (d && d.hoverKey !== hoverKey ? { ...d, hoverKey } : d));
	}

	function handleDragEnd(e: PointerEvent) {
		const key = ghost ? targetKeyAt(e.clientX, e.clientY) : null;
		endDrag();
		if (key) dispatch('assign', key);
	}

	function endDrag() {
		if (ghost) {
			document.body.style.cursor = '';
			modulationDrag.set(null);
		}
		dragStart = null;
		ghost = null;
	}

	onDestroy(() => {
		stopPlayhead();
		endDrag();
	});
</script>

<div class="card" style:--lfo-color={color}>
	<div class="head">
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<div
			class="handle"
			class:dragging={ghost !== null}
			title="Glisser sur un réglage pour l'assigner"
			on:pointerdown={handleDragStart}
			on:pointermove={handleDragMove}
			on:pointerup={handleDragEnd}
			on:pointercancel={endDrag}
		>
			<svg width="14" height="8" viewBox="-1 -1 16 10" aria-hidden="true">
				<path d={SHAPES.find((s) => s.id === shape)?.icon} />
			</svg>
		</div>
		<span class="name">{label}</span>
		<button class="assign-btn" type="button" class:active={pickerOpen}
			on:mousedown|stopPropagation
			on:click|stopPropagation={() => dispatch('togglepicker')}
			title="Choisir un réglage dans la liste">Assigner</button>
		<button class="rm-btn" type="button" on:click|stopPropagation={() => dispatch('remove')} title="Supprimer">×</button>
	</div>

	<div class="display">
		<svg viewBox="0 0 100 32" preserveAspectRatio="none" aria-hidden="true">
			<defs>
				<linearGradient id="lfo-fill-{mod.id}" x1="0" y1="0" x2="0" y2="1">
					<stop offset="0" stop-color={color} stop-opacity="0.35" />
					<stop offset="0.5" stop-color={color} stop-opacity="0.05" />
					<stop offset="1" stop-color={color} stop-opacity="0.35" />
				</linearGradient>
			</defs>
			<line class="axis" x1="0" y1={MID_Y} x2="100" y2={MID_Y} />
			<path class="area" d={area} fill="url(#lfo-fill-{mod.id})" />
			<path class="curve" d={line} />
			{#if playing}
				<line class="cursor" x1={phase * 100} y1="0" x2={phase * 100} y2="32" />
			{/if}
		</svg>
		{#if playing}
			<span class="dot" style:left="{phase * 100}%" style:top="{dotTopPercent}%"></span>
		{/if}
		<span class="rate-readout">{rateLabel}</span>
	</div>

	<div class="controls">
		<div class="shapes" role="radiogroup" aria-label="Forme d'onde">
			{#each SHAPES as s (s.id)}
				<button
					type="button"
					class="shape-btn"
					class:active={shape === s.id}
					role="radio"
					aria-checked={shape === s.id}
					title={s.label}
					on:click={() => shape !== s.id && update({ waveform: s.id })}
				>
					<svg width="14" height="8" viewBox="-1 -1 16 10" aria-hidden="true"><path d={s.icon} /></svg>
				</button>
			{/each}
		</div>
		<button
			type="button"
			class="sync-btn"
			class:active={sync}
			title={sync ? 'Vitesse libre (Hz)' : 'Caler sur le tempo'}
			on:click={() => update({ tempo_sync: !sync })}
		>{sync ? 'SYNC' : 'HZ'}</button>
		{#key sync}
			<Knob
				min={0}
				max={sync ? DIVISIONS.length - 1 : 1}
				defaultValue={sync ? 2 : rateToKnob(1)}
				pixelRange={sync ? 120 : 200}
				bind:value={knobValue}
				{color}
				on:release={(e) => handleKnobRelease(e.detail)}
			/>
		{/key}
	</div>
</div>

{#if ghost}
	<div class="drag-ghost" style:left="{ghost.x + 12}px" style:top="{ghost.y + 12}px" style:background={color}>
		{label}
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.card {
		padding: 5px 6px 6px 4px;
		border-left: 2px solid var(--lfo-color);
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.head {
		display: flex;
		align-items: center;
		gap: 5px;
	}

	.handle {
		flex-shrink: 0;
		width: 20px;
		height: 16px;
		display: flex;
		align-items: center;
		justify-content: center;
		border-radius: 3px;
		cursor: grab;
		touch-action: none;
		color: var(--lfo-color);
		&:hover { background: color-mix(in srgb, var(--lfo-color) 18%, transparent); }
		&.dragging { cursor: grabbing; opacity: 0.5; }
	}

	.handle path,
	.shape-btn path {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.3;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.name {
		flex: 1;
		min-width: 0;
		font-size: 10px;
		font-weight: 600;
		color: var(--lfo-color);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		user-select: none;
	}

	.assign-btn {
		flex-shrink: 0;
		padding: 1px 5px;
		border: 1px solid $border-subtle;
		border-radius: 3px;
		background: transparent;
		color: $text-subtle;
		font-size: 9px;
		line-height: 1.3;
		cursor: pointer;
		&:hover, &.active {
			background: color-mix(in srgb, var(--lfo-color) 15%, transparent);
			color: $text-color;
			border-color: color-mix(in srgb, var(--lfo-color) 45%, transparent);
		}
	}

	.rm-btn {
		width: 16px; height: 16px; border: none; background: transparent;
		color: $text-faint; font-size: 14px; line-height: 1; cursor: pointer;
		padding: 0; display: flex; align-items: center; justify-content: center;
		flex-shrink: 0;
		&:hover { color: #ff4136; }
	}

	.display {
		position: relative;
		height: 36px;
		border-radius: 4px;
		background: rgba(0, 0, 0, 0.3);
		border: 1px solid $border-faint;
		overflow: hidden;

		svg {
			width: 100%;
			height: 100%;
			display: block;
		}

		.axis {
			stroke: $border-faint;
			stroke-width: 1;
			stroke-dasharray: 2 2;
			vector-effect: non-scaling-stroke;
		}

		.area {
			stroke: none;
		}

		.curve {
			stroke: var(--lfo-color);
			stroke-width: 1.5;
			vector-effect: non-scaling-stroke;
		}

		.cursor {
			stroke: var(--lfo-color);
			stroke-width: 1;
			stroke-opacity: 0.35;
			vector-effect: non-scaling-stroke;
		}
	}

	.dot {
		position: absolute;
		width: 6px;
		height: 6px;
		margin: -3px 0 0 -3px;
		border-radius: 50%;
		background: #fff;
		box-shadow: 0 0 6px var(--lfo-color), 0 0 0 1.5px var(--lfo-color);
		pointer-events: none;
	}

	.rate-readout {
		position: absolute;
		top: 2px;
		right: 4px;
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		color: $text-subtle;
		text-shadow: 0 0 3px rgba(0, 0, 0, 0.8);
		pointer-events: none;
	}

	.controls {
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.shapes {
		flex: 1;
		display: flex;
		gap: 2px;
		min-width: 0;
	}

	.shape-btn {
		flex: 1;
		max-width: 24px;
		height: 18px;
		padding: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		border: 1px solid $border-faint;
		border-radius: 3px;
		background: transparent;
		color: $text-faint;
		cursor: pointer;
		&:hover { color: $text-color; border-color: $border-subtle; }
		&.active {
			color: var(--lfo-color);
			border-color: color-mix(in srgb, var(--lfo-color) 55%, transparent);
			background: color-mix(in srgb, var(--lfo-color) 14%, transparent);
		}
	}

	.sync-btn {
		flex-shrink: 0;
		height: 18px;
		padding: 0 4px;
		border: 1px solid $border-faint;
		border-radius: 3px;
		background: transparent;
		color: $text-faint;
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		cursor: pointer;
		&:hover { color: $text-color; border-color: $border-subtle; }
		&.active {
			color: var(--lfo-color);
			border-color: color-mix(in srgb, var(--lfo-color) 55%, transparent);
			background: color-mix(in srgb, var(--lfo-color) 14%, transparent);
		}
	}

	.drag-ghost {
		position: fixed;
		z-index: 1000;
		pointer-events: none;
		padding: 2px 7px;
		border-radius: 10px;
		font-size: 9px;
		font-family: $font-mono;
		font-weight: 700;
		color: #fff;
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.5);
		white-space: nowrap;
	}
</style>
