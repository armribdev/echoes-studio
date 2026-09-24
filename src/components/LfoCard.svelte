<!-- LfoCard.svelte — LFO body: live waveform display, shape, rate / tempo sync -->
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
	import { onDestroy } from 'svelte';
	import Knob from './Knob.svelte';
	import ModulatorCard from './ModulatorCard.svelte';
	import { tempo } from '../stores/tempo';
	import { updateModulatorOrNotify } from '../frontend/modulation/modulators';
	import type { LfoWaveform, ProjectModulator } from '../types/global';

	export let trackId: string;
	export let mod: ProjectModulator;
	export let label: string;
	export let color: string;
	export let pickerOpen = false;

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

	function update(changes: Parameters<typeof updateModulatorOrNotify>[2]) {
		updateModulatorOrNotify(trackId, mod.id, changes);
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

	onDestroy(stopPlayhead);
</script>

<ModulatorCard
	modulatorId={mod.id}
	{label}
	{color}
	icon={SHAPES.find((s) => s.id === shape)?.icon ?? SHAPES[0].icon}
	{pickerOpen}
	on:assign
	on:togglepicker
	on:remove
>
	<div class="mod-display">
		<svg viewBox="0 0 100 32" preserveAspectRatio="none" aria-hidden="true">
			<defs>
				<linearGradient id="lfo-fill-{mod.id}" x1="0" y1="0" x2="0" y2="1">
					<stop offset="0" stop-color={color} stop-opacity="0.35" />
					<stop offset="0.5" stop-color={color} stop-opacity="0.05" />
					<stop offset="1" stop-color={color} stop-opacity="0.35" />
				</linearGradient>
			</defs>
			<line class="axis" x1="0" y1={MID_Y} x2="100" y2={MID_Y} />
			<path d={area} fill="url(#lfo-fill-{mod.id})" />
			<path class="curve" d={line} />
			{#if playing}
				<line class="cursor" x1={phase * 100} y1="0" x2={phase * 100} y2="32" />
			{/if}
		</svg>
		{#if playing}
			<span class="dot" style:left="{phase * 100}%" style:top="{dotTopPercent}%"></span>
		{/if}
		<span class="mod-readout">{rateLabel}</span>
	</div>

	<div class="mod-controls lfo-controls">
		<div class="shapes" role="radiogroup" aria-label="Forme d'onde">
			{#each SHAPES as s (s.id)}
				<button
					type="button"
					class="mod-toggle shape-btn"
					class:active={shape === s.id}
					role="radio"
					aria-checked={shape === s.id}
					title={s.label}
					on:click={() => shape !== s.id && update({ waveform: s.id })}
				>
					<svg width="14" height="8" viewBox="-1 -1 16 10" aria-hidden="true"><path class="icon" d={s.icon} /></svg>
				</button>
			{/each}
		</div>
		<button
			type="button"
			class="mod-toggle"
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
</ModulatorCard>

<style lang="scss">
	.cursor {
		stroke: var(--mod-color);
		stroke-width: 1;
		stroke-opacity: 0.35;
		vector-effect: non-scaling-stroke;
	}

	.dot {
		position: absolute;
		width: 6px;
		height: 6px;
		margin: -3px 0 0 -3px;
		border-radius: 50%;
		background: #fff;
		box-shadow: 0 0 6px var(--mod-color), 0 0 0 1.5px var(--mod-color);
		pointer-events: none;
	}

	.lfo-controls {
		align-items: center;
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
		padding: 0;
	}
</style>
