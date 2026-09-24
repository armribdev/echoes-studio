<!-- FollowerCard.svelte — audio follower body: level history display, RMS/peak mode, window/attack/release -->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import ModulatorCard from './ModulatorCard.svelte';
	import ParamKnob from './ParamKnob.svelte';
	import { trackLevels } from '../stores/trackLevels';
	import { formatMs, kindInfo, updateModulatorOrNotify } from '../frontend/modulation/modulators';
	import type { FollowerMode, ProjectModulator } from '../types/global';

	export let trackId: string;
	export let mod: ProjectModulator;
	export let label: string;
	export let color: string;
	export let pickerOpen = false;

	const MODES: { id: FollowerMode; label: string; title: string }[] = [
		{ id: 'rms', label: 'RMS', title: 'Volume moyen (plus doux)' },
		{ id: 'peak', label: 'PEAK', title: 'Crêtes (plus nerveux)' },
	];

	// Defaults mirror `AudioFollowerParams::default()` in the engine.
	$: mode = mod.mode ?? 'rms';
	let attack = mod.attack_ms ?? 10;
	let release = mod.release_ms ?? 100;

	// Indicative preview: the track meter (~30 updates/s) smoothed like the engine
	// follower would, so attack and release are visible. Not the engine's output.
	const HISTORY_LENGTH = 90;
	const FLOOR_DB = -60;
	const METER_INTERVAL_MS = 1000 / 30;

	let raw: number[] = Array(HISTORY_LENGTH).fill(0);
	let smoothed: number[] = Array(HISTORY_LENGTH).fill(0);
	let level = 0;

	function toDisplay(peak: number): number {
		if (peak <= 0) return 0;
		return Math.max(0, (20 * Math.log10(peak) - FLOOR_DB) / -FLOOR_DB);
	}

	function smoothingCoefficient(ms: number): number {
		return ms <= 0 ? 1 : 1 - Math.exp(-METER_INTERVAL_MS / ms);
	}

	const unsubscribe = trackLevels.subscribe((levels) => {
		const peaks = levels[trackId];
		const instant = peaks ? toDisplay(Math.max(peaks[0], peaks[1])) : 0;
		const coefficient = smoothingCoefficient(instant > level ? attack : release);
		level += coefficient * (instant - level);
		raw = [...raw.slice(1), instant];
		smoothed = [...smoothed.slice(1), level];
	});

	onDestroy(unsubscribe);

	function polyline(values: number[]): string {
		return values
			.map((v, i) => `${(i / (HISTORY_LENGTH - 1)) * 100},${30 - v * 27}`)
			.join(' ');
	}

	function update(changes: Parameters<typeof updateModulatorOrNotify>[2]) {
		updateModulatorOrNotify(trackId, mod.id, changes);
	}
</script>

<ModulatorCard
	modulatorId={mod.id}
	{label}
	{color}
	icon={kindInfo('audio_follower').icon}
	{pickerOpen}
	on:assign
	on:togglepicker
	on:remove
>
	<div class="mod-display">
		<svg viewBox="0 0 100 32" preserveAspectRatio="none" aria-hidden="true">
			<line class="axis" x1="0" y1="30" x2="100" y2="30" />
			<polyline class="raw" points={polyline(raw)} />
			<polyline class="curve" points={polyline(smoothed)} />
		</svg>
		<span class="mod-readout">Niveau de la piste</span>
	</div>

	<div class="mod-controls">
		<div class="modes" role="radiogroup" aria-label="Mode de détection">
			{#each MODES as m (m.id)}
				<button
					type="button"
					class="mod-toggle"
					class:active={mode === m.id}
					role="radio"
					aria-checked={mode === m.id}
					title={m.title}
					on:click={() => mode !== m.id && update({ mode: m.id })}
				>{m.label}</button>
			{/each}
		</div>
		<ParamKnob label="Fen." value={mod.window_ms ?? 50} min={5} max={500} defaultValue={50}
			scale="log" format={formatMs} {color} on:release={(e) => update({ window_ms: e.detail })} />
		<ParamKnob label="Att." value={mod.attack_ms ?? 10} bind:live={attack} min={0.1} max={500} defaultValue={10}
			scale="log" format={formatMs} {color} on:release={(e) => update({ attack_ms: e.detail })} />
		<ParamKnob label="Rel." value={mod.release_ms ?? 100} bind:live={release} min={1} max={2000} defaultValue={100}
			scale="log" format={formatMs} {color} on:release={(e) => update({ release_ms: e.detail })} />
	</div>
</ModulatorCard>

<style lang="scss">
	.raw {
		fill: none;
		stroke: rgba(255, 255, 255, 0.25);
		stroke-width: 1;
		vector-effect: non-scaling-stroke;
	}

	.modes {
		display: flex;
		flex-direction: column;
		gap: 2px;
		align-self: center;
	}
</style>
