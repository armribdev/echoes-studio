<!-- ArpeggiatorPanel.svelte — arpeggiator note stage of an instrument track (ADR-030) -->
<script lang="ts">
	import ParamKnob from './ParamKnob.svelte';
	import CustomSelect from './CustomSelect.svelte';
	import type { ArpMode, ArpRate, ArpeggiatorSettings } from '../types/global';
	import type { ArpeggiatorPatch } from '../shared/protocol/commands';
	import {
		ARP_MODE_OPTIONS,
		ARP_RATE_OPTIONS,
		DEFAULT_ARPEGGIATOR
	} from '../shared/project/arpeggiator';
	import { setArpeggiator } from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';

	export let trackId: string;
	export let arpeggiator: ArpeggiatorSettings | null = null;

	$: settings = arpeggiator ?? DEFAULT_ARPEGGIATOR;

	async function update(patch: ArpeggiatorPatch) {
		try {
			await setArpeggiator(trackId, patch);
		} catch (e) {
			notify(`Erreur arpégiateur : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	function setMode(mode: string | number) {
		update({ mode: mode as ArpMode });
	}

	function setRate(rate: string | number) {
		update({ rate: rate as ArpRate });
	}

	const formatPercent = (v: number) => `${Math.round(v)} %`;
	const formatOctaves = (v: number) => `${Math.round(v)}`;
</script>

<div class="arp" class:off={!settings.enabled}>
	<div class="arp-head">
		<button
			class="power"
			class:on={settings.enabled}
			type="button"
			aria-pressed={settings.enabled}
			title="Arpégiateur"
			on:click={() => update({ enabled: !settings.enabled })}
		>
			<svg width="8" height="8" viewBox="0 0 8 8"
				><circle cx="4" cy="4" r="3" fill="currentColor" /></svg
			>
		</button>
		<span class="arp-title">Arpégiateur</span>
	</div>

	<div class="arp-selects">
		<CustomSelect
			small
			size="sm"
			value={settings.mode}
			options={ARP_MODE_OPTIONS}
			on:change={(e) => setMode(e.detail)}
		/>
		<CustomSelect
			small
			size="sm"
			value={settings.rate}
			options={ARP_RATE_OPTIONS}
			on:change={(e) => setRate(e.detail)}
		/>
		<button
			class="triplet"
			class:active={settings.triplet}
			type="button"
			aria-pressed={settings.triplet}
			title="Triolet"
			on:click={() => update({ triplet: !settings.triplet })}>3</button
		>
	</div>

	<div class="arp-knobs">
		<ParamKnob
			label="Gate"
			value={settings.gate_percent}
			min={5}
			max={100}
			defaultValue={80}
			format={formatPercent}
			on:release={(e) => update({ gate_percent: Math.round(e.detail) })}
		/>
		<ParamKnob
			label="Oct"
			value={settings.octaves}
			min={1}
			max={4}
			defaultValue={1}
			format={formatOctaves}
			on:release={(e) => update({ octaves: Math.round(e.detail) })}
		/>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.arp {
		border-top: 1px solid $border-faint;
		border-bottom: 1px solid $border-faint;

		&.off .arp-knobs,
		&.off .triplet,
		&.off .arp-selects :global(.cs-trigger) {
			opacity: 0.5;
		}
	}

	.arp-head {
		display: flex;
		align-items: center;
		gap: $space-2;
		padding: $space-2 $panel-inset;
		background: $panel-background-color;
	}

	.power {
		@include power-button;
	}

	.arp-title {
		flex: 1;
		font-size: 10px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.08em;
		color: $text-muted;
		text-transform: uppercase;
	}

	.arp-selects {
		display: flex;
		gap: $space-2;
		padding: $space-3 $panel-inset 0;

		:global(> *:not(.triplet)) {
			flex: 1;
			min-width: 0;
		}
	}

	.triplet {
		@include button($ctrl-height-sm, 10px);
		flex-shrink: 0;
		width: 24px;
		padding: 0;
		font-family: $font-mono;
		font-weight: 700;

		&.active {
			@include control-on;
		}
	}

	.arp-knobs {
		display: flex;
		align-items: flex-end;
		gap: $space-2;
		padding: $space-3 $panel-inset;
	}
</style>
