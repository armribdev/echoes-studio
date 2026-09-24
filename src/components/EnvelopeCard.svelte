<!-- EnvelopeCard.svelte — ADSR envelope body: shape display and A/D/S/R knobs -->
<script lang="ts">
	import ModulatorCard from './ModulatorCard.svelte';
	import ParamKnob from './ParamKnob.svelte';
	import {
		formatMs,
		formatPercent,
		kindInfo,
		updateModulatorOrNotify,
	} from '../frontend/modulation/modulators';
	import type { ProjectModulator } from '../types/global';

	export let trackId: string;
	export let mod: ProjectModulator;
	export let label: string;
	export let color: string;
	export let pickerOpen = false;
	/** The envelope is gated by the notes of the track's instrument. */
	export let hasInstrument: boolean;

	// Defaults mirror `EnvelopeParams::default()` in the engine.
	let attack = mod.attack_ms ?? 10;
	let decay = mod.decay_ms ?? 100;
	let sustain = mod.sustain_level ?? 0.7;
	let release = mod.release_ms ?? 200;

	// Display geometry, in viewBox units (100 × 32). The sustain plateau has a fixed width.
	const TOP_Y = 3;
	const BOTTOM_Y = 30;
	const SUSTAIN_WIDTH = 22;
	const MIN_SEGMENT_WIDTH = 3;

	function envelopePath(a: number, d: number, s: number, r: number): string {
		const weights = [a, d, r].map(Math.sqrt);
		const total = weights.reduce((sum, w) => sum + w, 0) || 1;
		const [wa, wd, wr] = weights.map((w) => MIN_SEGMENT_WIDTH + (w / total) * (100 - SUSTAIN_WIDTH - 3 * MIN_SEGMENT_WIDTH));
		const sustainY = BOTTOM_Y - s * (BOTTOM_Y - TOP_Y);
		const xa = wa;
		const xd = xa + wd;
		const xs = xd + SUSTAIN_WIDTH;
		const xr = xs + wr;
		return `M0,${BOTTOM_Y} L${xa},${TOP_Y} L${xd},${sustainY} L${xs},${sustainY} L${xr},${BOTTOM_Y}`;
	}

	$: line = envelopePath(attack, decay, sustain, release);
	$: area = `${line} Z`;

	function update(changes: Parameters<typeof updateModulatorOrNotify>[2]) {
		updateModulatorOrNotify(trackId, mod.id, changes);
	}
</script>

<ModulatorCard
	modulatorId={mod.id}
	{label}
	{color}
	icon={kindInfo('envelope').icon}
	{pickerOpen}
	on:assign
	on:togglepicker
	on:remove
>
	<div class="mod-display">
		<svg viewBox="0 0 100 32" preserveAspectRatio="none" aria-hidden="true">
			<defs>
				<linearGradient id="env-fill-{mod.id}" x1="0" y1="0" x2="0" y2="1">
					<stop offset="0" stop-color={color} stop-opacity="0.35" />
					<stop offset="1" stop-color={color} stop-opacity="0.03" />
				</linearGradient>
			</defs>
			<line class="axis" x1="0" y1={BOTTOM_Y} x2="100" y2={BOTTOM_Y} />
			<path d={area} fill="url(#env-fill-{mod.id})" />
			<path class="curve" d={line} />
		</svg>
	</div>

	<div class="mod-controls">
		<ParamKnob label="A" value={mod.attack_ms ?? 10} bind:live={attack} min={1} max={5000} defaultValue={10}
			scale="log" format={formatMs} {color} on:release={(e) => update({ attack_ms: e.detail })} />
		<ParamKnob label="D" value={mod.decay_ms ?? 100} bind:live={decay} min={1} max={5000} defaultValue={100}
			scale="log" format={formatMs} {color} on:release={(e) => update({ decay_ms: e.detail })} />
		<ParamKnob label="S" value={mod.sustain_level ?? 0.7} bind:live={sustain} min={0} max={1} defaultValue={0.7}
			format={formatPercent} {color} on:release={(e) => update({ sustain_level: e.detail })} />
		<ParamKnob label="R" value={mod.release_ms ?? 200} bind:live={release} min={1} max={10000} defaultValue={200}
			scale="log" format={formatMs} {color} on:release={(e) => update({ release_ms: e.detail })} />
	</div>

	{#if !hasInstrument}
		<p class="mod-hint">Se déclenche avec les notes MIDI : ajoutez un instrument à cette piste.</p>
	{/if}
</ModulatorCard>
