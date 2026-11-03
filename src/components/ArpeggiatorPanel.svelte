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
			value={settings.mode}
			options={ARP_MODE_OPTIONS}
			on:change={(e) => setMode(e.detail)}
		/>
		<CustomSelect
			small
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

	.arp {
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
		gap: 5px;
		padding: 5px 6px;
		background: #222229;
	}

	.power {
		width: 13px;
		height: 13px;
		border-radius: 50%;
		border: 1px solid $border-subtle;
		background: transparent;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 0;
		flex-shrink: 0;
		color: $text-faint;

		&.on {
			color: $accent-color;
			border-color: $accent-color;
		}

		&:focus-visible {
			outline: 1px solid $accent-hover;
			outline-offset: 1px;
		}
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
		gap: 4px;
		padding: 6px 6px 2px;

		:global(> *:not(.triplet)) {
			flex: 1;
			min-width: 0;
		}
	}

	.triplet {
		flex-shrink: 0;
		width: 22px;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		background: transparent;
		color: $text-subtle;
		font-size: 10px;
		font-family: $font-mono;
		font-weight: 700;
		cursor: pointer;

		&:hover {
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;
		}

		&.active {
			background: rgba(21, 113, 255, 0.2);
			color: $accent-color;
			border-color: rgba(21, 113, 255, 0.5);
		}

		&:focus-visible {
			outline: 1px solid $accent-hover;
			outline-offset: 1px;
		}
	}

	.arp-knobs {
		display: flex;
		align-items: flex-end;
		gap: 4px;
		padding: 2px 6px 6px;
	}
</style>
