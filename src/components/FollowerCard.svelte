<!-- FollowerCard.svelte — audio follower body: level history display, source, RMS/peak mode, window/attack/release -->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import ModulatorCard from './ModulatorCard.svelte';
	import ParamKnob from './ParamKnob.svelte';
	import { trackLevels } from '../stores/trackLevels';
	import { formatMs, kindInfo, updateModulatorOrNotify } from '../frontend/modulation/modulators';
	import { effectGroupNames } from '../frontend/modulation/targets';
	import type { FollowerMode, ProjectModulator, TrackEffect } from '../types/global';

	export let trackId: string;
	export let mod: ProjectModulator;
	export let label: string;
	export let color: string;
	export let pickerOpen = false;
	export let effects: TrackEffect[];

	const POST_FX = '';

	const MODES: { id: FollowerMode; label: string; title: string }[] = [
		{ id: 'rms', label: 'RMS', title: 'Volume moyen (plus doux)' },
		{ id: 'peak', label: 'PEAK', title: 'Crêtes (plus nerveux)' }
	];

	// Defaults mirror `AudioFollowerParams::default()` in the engine.
	$: mode = mod.mode ?? 'rms';
	$: effectNames = effectGroupNames(effects);
	// A tap on a removed effect falls back to the post-FX output in the engine.
	$: source =
		mod.source_effect_id && effectNames.has(mod.source_effect_id) ? mod.source_effect_id : POST_FX;
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
		return values.map((v, i) => `${(i / (HISTORY_LENGTH - 1)) * 100},${30 - v * 27}`).join(' ');
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

	<label class="source">
		<span>Écoute</span>
		<select
			value={source}
			title="Point de la chaîne d'effets dont le suiveur mesure le volume"
			on:change={(e) => update({ source_effect_id: e.currentTarget.value || null })}
		>
			<option value={POST_FX}>Sortie de piste</option>
			{#each effects as effect (effect.id)}
				<option value={effect.id}>Après {effectNames.get(effect.id)}</option>
			{/each}
		</select>
	</label>

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
					on:click={() => mode !== m.id && update({ mode: m.id })}>{m.label}</button
				>
			{/each}
		</div>
		<ParamKnob
			label="Fen."
			value={mod.window_ms ?? 50}
			min={5}
			max={500}
			defaultValue={50}
			scale="log"
			format={formatMs}
			{color}
			on:release={(e) => update({ window_ms: e.detail })}
		/>
		<ParamKnob
			label="Att."
			value={mod.attack_ms ?? 10}
			bind:live={attack}
			min={0.1}
			max={500}
			defaultValue={10}
			scale="log"
			format={formatMs}
			{color}
			on:release={(e) => update({ attack_ms: e.detail })}
		/>
		<ParamKnob
			label="Rel."
			value={mod.release_ms ?? 100}
			bind:live={release}
			min={1}
			max={2000}
			defaultValue={100}
			scale="log"
			format={formatMs}
			{color}
			on:release={(e) => update({ release_ms: e.detail })}
		/>
	</div>
</ModulatorCard>

<style lang="scss">
	@use '../variables.scss' as *;

	.source {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 9px;
		color: $text-subtle;

		select {
			flex: 1;
			min-width: 0;
			height: 20px;
			padding: 0 4px;
			background: rgba(255, 255, 255, 0.04);
			border: 1px solid rgba(255, 255, 255, 0.08);
			border-radius: $radius-sm;
			color: #c8c8d8;
			font-size: 10px;
			cursor: pointer;

			&:hover {
				background: rgba(255, 255, 255, 0.07);
				border-color: rgba(255, 255, 255, 0.14);
			}
			&:focus-visible {
				outline: none;
				border-color: var(--mod-color);
			}
		}
	}

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
