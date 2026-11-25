<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import Knob from '../Knob.svelte';
	import { modulatorColor } from '../../frontend/modulation/colors';
	import { automatedTargets, automatedValues } from '../../stores/automation';
	import { selectedTrack } from '../../stores/selectedTrack';
	import type {
		TrackEffect,
		EqParams,
		CompressorParams,
		ReverbParams,
		DelayParams,
		ProjectModulator,
		ProjectModulationRoute
	} from '../../types/global';

	export let trackId: string;
	export let effect: TrackEffect;
	export let compact = false;
	export let modulators: ProjectModulator[] = [];
	export let mod_routes: ProjectModulationRoute[] = [];

	const dispatch = createEventDispatcher<{ change: Record<string, unknown> }>();

	$: automated = $automatedTargets[trackId];

	$: automatedNow = $automatedValues[trackId];

	function automatedValueFor(effectId: string, param: string): number | undefined {
		return automatedNow?.[`${effectId}/${param}`];
	}

	function automationColorFor(effectId: string, param: string): string | undefined {
		return automated?.has(`${effectId}/${param}`) ? $selectedTrack.trackColor : undefined;
	}

	function modColorFor(effectId: string, param: string): string | undefined {
		const route = mod_routes.find(
			(r) =>
				r.target.kind === 'effect_param' &&
				r.target.effect_id === effectId &&
				r.target.param === param
		);
		if (!route) return undefined;
		const idx = modulators.findIndex((m) => m.id === route.modulator_id);
		return modulatorColor(idx >= 0 ? idx : 0);
	}

	function eqP(e: TrackEffect) {
		return (e.params as { kind: 'eq'; params: EqParams }).params;
	}
	function compP(e: TrackEffect) {
		return (e.params as { kind: 'compressor'; params: CompressorParams }).params;
	}
	function revP(e: TrackEffect) {
		return (e.params as { kind: 'reverb'; params: ReverbParams }).params;
	}
	function delP(e: TrackEffect) {
		return (e.params as { kind: 'delay'; params: DelayParams }).params;
	}
</script>

{#if effect.params.kind === 'eq'}
	{@const p = eqP(effect)}
	<div class="grid" class:compact>
		<span class="lbl">LOW</span>
		<Knob
			min={-12}
			max={12}
			defaultValue={0}
			value={p.low_gain_db}
			modColor={modColorFor(effect.id, 'low_gain_db')}
			automationColor={automationColorFor(effect.id, 'low_gain_db')}
			automatedValue={automatedValueFor(effect.id, 'low_gain_db')}
			modTargetKey={`${effect.id}/low_gain_db`}
			on:release={(e) => dispatch('change', { ...p, low_gain_db: e.detail })}
		/>
		<span class="val">{p.low_gain_db.toFixed(1)} dB</span>
		<span class="lbl">MID</span>
		<Knob
			min={-12}
			max={12}
			defaultValue={0}
			value={p.mid_gain_db}
			modColor={modColorFor(effect.id, 'mid_gain_db')}
			automationColor={automationColorFor(effect.id, 'mid_gain_db')}
			automatedValue={automatedValueFor(effect.id, 'mid_gain_db')}
			modTargetKey={`${effect.id}/mid_gain_db`}
			on:release={(e) => dispatch('change', { ...p, mid_gain_db: e.detail })}
		/>
		<span class="val">{p.mid_gain_db.toFixed(1)} dB</span>
		<span class="lbl">HIGH</span>
		<Knob
			min={-12}
			max={12}
			defaultValue={0}
			value={p.high_gain_db}
			modColor={modColorFor(effect.id, 'high_gain_db')}
			automationColor={automationColorFor(effect.id, 'high_gain_db')}
			automatedValue={automatedValueFor(effect.id, 'high_gain_db')}
			modTargetKey={`${effect.id}/high_gain_db`}
			on:release={(e) => dispatch('change', { ...p, high_gain_db: e.detail })}
		/>
		<span class="val">{p.high_gain_db.toFixed(1)} dB</span>
		<span class="lbl">Q</span>
		<Knob
			min={0.1}
			max={10}
			defaultValue={1}
			value={p.mid_q}
			modColor={modColorFor(effect.id, 'mid_q')}
			automationColor={automationColorFor(effect.id, 'mid_q')}
			automatedValue={automatedValueFor(effect.id, 'mid_q')}
			modTargetKey={`${effect.id}/mid_q`}
			on:release={(e) => dispatch('change', { ...p, mid_q: e.detail })}
		/>
		<span class="val">{p.mid_q.toFixed(2)}</span>
	</div>
{:else if effect.params.kind === 'compressor'}
	{@const p = compP(effect)}
	<div class="grid" class:compact>
		<span class="lbl">THR</span>
		<Knob
			min={-60}
			max={0}
			defaultValue={-18}
			value={p.threshold_db}
			modColor={modColorFor(effect.id, 'threshold_db')}
			automationColor={automationColorFor(effect.id, 'threshold_db')}
			automatedValue={automatedValueFor(effect.id, 'threshold_db')}
			modTargetKey={`${effect.id}/threshold_db`}
			on:release={(e) => dispatch('change', { ...p, threshold_db: e.detail })}
		/>
		<span class="val">{p.threshold_db.toFixed(1)} dB</span>
		<span class="lbl">RATIO</span>
		<Knob
			min={1}
			max={20}
			defaultValue={4}
			value={p.ratio}
			modColor={modColorFor(effect.id, 'ratio')}
			automationColor={automationColorFor(effect.id, 'ratio')}
			automatedValue={automatedValueFor(effect.id, 'ratio')}
			modTargetKey={`${effect.id}/ratio`}
			on:release={(e) => dispatch('change', { ...p, ratio: e.detail })}
		/>
		<span class="val">{p.ratio.toFixed(1)}:1</span>
		<span class="lbl">ATK</span>
		<Knob
			min={0.1}
			max={200}
			defaultValue={10}
			value={p.attack_ms}
			modColor={modColorFor(effect.id, 'attack_ms')}
			automationColor={automationColorFor(effect.id, 'attack_ms')}
			automatedValue={automatedValueFor(effect.id, 'attack_ms')}
			modTargetKey={`${effect.id}/attack_ms`}
			on:release={(e) => dispatch('change', { ...p, attack_ms: e.detail })}
		/>
		<span class="val">{p.attack_ms.toFixed(0)} ms</span>
		<span class="lbl">REL</span>
		<Knob
			min={10}
			max={2000}
			defaultValue={100}
			value={p.release_ms}
			modColor={modColorFor(effect.id, 'release_ms')}
			automationColor={automationColorFor(effect.id, 'release_ms')}
			automatedValue={automatedValueFor(effect.id, 'release_ms')}
			modTargetKey={`${effect.id}/release_ms`}
			on:release={(e) => dispatch('change', { ...p, release_ms: e.detail })}
		/>
		<span class="val">{p.release_ms.toFixed(0)} ms</span>
		<span class="lbl">MKUP</span>
		<Knob
			min={-6}
			max={24}
			defaultValue={0}
			value={p.makeup_db}
			modColor={modColorFor(effect.id, 'makeup_db')}
			automationColor={automationColorFor(effect.id, 'makeup_db')}
			automatedValue={automatedValueFor(effect.id, 'makeup_db')}
			modTargetKey={`${effect.id}/makeup_db`}
			on:release={(e) => dispatch('change', { ...p, makeup_db: e.detail })}
		/>
		<span class="val">{p.makeup_db.toFixed(1)} dB</span>
	</div>
{:else if effect.params.kind === 'reverb'}
	{@const p = revP(effect)}
	<div class="grid" class:compact>
		<span class="lbl">SIZE</span>
		<Knob
			min={0}
			max={1}
			defaultValue={0.5}
			value={p.room_size}
			modColor={modColorFor(effect.id, 'room_size')}
			automationColor={automationColorFor(effect.id, 'room_size')}
			automatedValue={automatedValueFor(effect.id, 'room_size')}
			modTargetKey={`${effect.id}/room_size`}
			on:release={(e) => dispatch('change', { ...p, room_size: e.detail })}
		/>
		<span class="val">{(p.room_size * 100).toFixed(0)}%</span>
		<span class="lbl">DAMP</span>
		<Knob
			min={0}
			max={1}
			defaultValue={0.5}
			value={p.damping}
			modColor={modColorFor(effect.id, 'damping')}
			automationColor={automationColorFor(effect.id, 'damping')}
			automatedValue={automatedValueFor(effect.id, 'damping')}
			modTargetKey={`${effect.id}/damping`}
			on:release={(e) => dispatch('change', { ...p, damping: e.detail })}
		/>
		<span class="val">{(p.damping * 100).toFixed(0)}%</span>
		<span class="lbl">WET</span>
		<Knob
			min={0}
			max={1}
			defaultValue={0.33}
			value={p.wet}
			modColor={modColorFor(effect.id, 'wet')}
			automationColor={automationColorFor(effect.id, 'wet')}
			automatedValue={automatedValueFor(effect.id, 'wet')}
			modTargetKey={`${effect.id}/wet`}
			on:release={(e) => dispatch('change', { ...p, wet: e.detail })}
		/>
		<span class="val">{(p.wet * 100).toFixed(0)}%</span>
		<span class="lbl">WIDTH</span>
		<Knob
			min={0}
			max={1}
			defaultValue={1}
			value={p.width}
			modColor={modColorFor(effect.id, 'width')}
			automationColor={automationColorFor(effect.id, 'width')}
			automatedValue={automatedValueFor(effect.id, 'width')}
			modTargetKey={`${effect.id}/width`}
			on:release={(e) => dispatch('change', { ...p, width: e.detail })}
		/>
		<span class="val">{(p.width * 100).toFixed(0)}%</span>
	</div>
{:else if effect.params.kind === 'delay'}
	{@const p = delP(effect)}
	<div class="grid" class:compact>
		<span class="lbl">TIME</span>
		<Knob
			min={10}
			max={2000}
			defaultValue={250}
			value={p.delay_ms}
			modColor={modColorFor(effect.id, 'delay_ms')}
			automationColor={automationColorFor(effect.id, 'delay_ms')}
			automatedValue={automatedValueFor(effect.id, 'delay_ms')}
			modTargetKey={`${effect.id}/delay_ms`}
			on:release={(e) => dispatch('change', { ...p, delay_ms: e.detail })}
		/>
		<span class="val">{p.delay_ms.toFixed(0)} ms</span>
		<span class="lbl">FDBK</span>
		<Knob
			min={0}
			max={0.95}
			defaultValue={0.4}
			value={p.feedback}
			modColor={modColorFor(effect.id, 'feedback')}
			automationColor={automationColorFor(effect.id, 'feedback')}
			automatedValue={automatedValueFor(effect.id, 'feedback')}
			modTargetKey={`${effect.id}/feedback`}
			on:release={(e) => dispatch('change', { ...p, feedback: e.detail })}
		/>
		<span class="val">{(p.feedback * 100).toFixed(0)}%</span>
		<span class="lbl">WET</span>
		<Knob
			min={0}
			max={1}
			defaultValue={0.3}
			value={p.wet}
			modColor={modColorFor(effect.id, 'wet')}
			automationColor={automationColorFor(effect.id, 'wet')}
			automatedValue={automatedValueFor(effect.id, 'wet')}
			modTargetKey={`${effect.id}/wet`}
			on:release={(e) => dispatch('change', { ...p, wet: e.detail })}
		/>
		<span class="val">{(p.wet * 100).toFixed(0)}%</span>
		<span class="lbl">PING</span>
		<button
			class="toggle"
			class:on={p.ping_pong}
			type="button"
			on:click={() => dispatch('change', { ...p, ping_pong: !p.ping_pong })}
		>
			{p.ping_pong ? 'ON' : 'OFF'}
		</button>
		<span class="val"></span>
	</div>
{/if}

<style lang="scss">
	@use '../../variables.scss' as *;

	.grid.compact {
		grid-template-columns: 28px 26px 1fr;
		row-gap: 2px;

		.lbl {
			font-size: 7px;
		}
		.val {
			font-size: 8px;
		}
	}

	.grid {
		display: grid;
		grid-template-columns: 36px 28px 1fr;
		row-gap: 4px;
		column-gap: 4px;
		align-items: center;
	}

	.lbl {
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		color: $text-faint;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		text-align: right;
		user-select: none;
	}

	.val {
		font-size: 9px;
		font-family: $font-mono;
		color: $text-subtle;
		padding-left: 2px;
	}

	.toggle {
		height: 18px;
		padding: 0 5px;
		border: 1px solid $border-subtle;
		border-radius: $radius-xs;
		background: transparent;
		color: $text-subtle;
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		cursor: pointer;
		&.on {
			background: $accent-strong;
			border-color: $accent-border;
			color: $accent-color;
		}
	}
</style>
