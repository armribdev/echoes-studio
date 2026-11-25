<script lang="ts">
	import { t } from 'svelte-i18n';
	import CustomSelect from '../CustomSelect.svelte';
	import Field from '../Field.svelte';
	import FieldGrid from '../FieldGrid.svelte';
	import Icon from '../Icon.svelte';
	import PillGroup from '../PillGroup.svelte';
	import StepSlider from '../StepSlider.svelte';
	import SubSection from '../SubSection.svelte';
	import Toggle from '../Toggle.svelte';
	import { plugins } from '../../stores/plugins';
	import type { Preferences } from '../../stores/preferences';

	export let audio: Preferences['audio'];
	export let sampleRateSliderOptions: {
		value: Preferences['audio']['sampleRate'];
		label: string;
	}[];
	export let xrunCount: number;
	export let realOutputChannels: number;

	// Routing state (local, not persisted yet)
	let masterRouting = 'out-1-2';
	let headphoneRouting = 'mirror';

	const BUFFER_SIZES = [64, 128, 256, 512, 1024, 2048] as const;

	$: latencyMs = (audio.bufferSize / audio.sampleRate) * 1000;

	$: isolationOptions = [
		{ value: 'off' as const, label: $t('common.none') },
		{ value: 'chosen' as const, label: $t('common.chosen') },
		{ value: 'all' as const, label: $t('common.all') }
	];

	// Buffer size slider options (always all 6 — fixed-buffer failures are
	// handled at stream build time, not at enumeration time).
	const bufferSliderOptions = BUFFER_SIZES.map((bs) => ({
		value: bs,
		label: String(bs)
	}));
</script>

<SubSection title={$t('settings.audio.qualityTitle')}>
	<FieldGrid>
		<Field label={$t('settings.audio.sampleRate')}>
			{#if sampleRateSliderOptions.length <= 1}
				<div class="single-value-display">
					{sampleRateSliderOptions[0]?.label ??
						(audio.sampleRate >= 1000
							? audio.sampleRate / 1000 + ' kHz'
							: audio.sampleRate + ' Hz')}
				</div>
				<span class="field-hint">{$t('settings.audio.sampleRateSingleHint')}</span>
			{:else}
				<StepSlider bind:value={audio.sampleRate} options={sampleRateSliderOptions} />
			{/if}
		</Field>

		<Field label={$t('settings.audio.bufferSize')}>
			<StepSlider bind:value={audio.bufferSize} options={bufferSliderOptions} />
		</Field>

		<Field label={$t('settings.audio.bitDepth')} hint={$t('settings.audio.bitDepthHint')}>
			<div class="single-value-display">
				<Icon name="cpu" size={14} />
				32-bit float
			</div>
		</Field>
	</FieldGrid>

	<div class="latency-bar">
		<span class="latency-label">{$t('settings.audio.estimatedLatency')}</span>
		<span class="latency-value">{latencyMs.toFixed(2)} ms</span>
		<div class="latency-indicator">
			<div
				class="latency-fill"
				class:latency-low={latencyMs < 10}
				class:latency-mid={latencyMs >= 10 && latencyMs < 25}
				class:latency-high={latencyMs >= 25}
				style="width: {Math.min((latencyMs / 50) * 100, 100)}%"
			></div>
		</div>
		<span class="latency-hint">
			{#if latencyMs < 10}{$t('settings.audio.latencyExcellent')}
			{:else if latencyMs < 25}{$t('settings.audio.latencyGood')}
			{:else}{$t('settings.audio.latencyHigh')}{/if}
		</span>
	</div>

	<div class="xrun-bar" class:xrun-warning={xrunCount > 0}>
		<span class="xrun-label">{$t('settings.audio.xruns')}</span>
		<span class="xrun-value">{xrunCount}</span>
		<span class="xrun-hint">
			{#if xrunCount === 0}{$t('settings.audio.xrunsNone')}
			{:else}{$t('settings.audio.xrunsHint')}{/if}
		</span>
	</div>
</SubSection>
<SubSection title={$t('settings.audio.masterOutTitle')}>
	<Toggle
		bind:checked={audio.masterSoftClip}
		label={$t('settings.audio.softClip')}
		description={$t('settings.audio.softClipDesc')}
	/>
</SubSection>
<SubSection title={$t('settings.audio.isolationTitle')}>
	<Field label={$t('settings.audio.isolatedPlugins')}>
		<PillGroup bind:value={audio.pluginIsolation} options={isolationOptions} />
		<span slot="hint">
			{#if audio.pluginIsolation === 'off'}{$t('settings.audio.isolationOff')}
			{:else if audio.pluginIsolation === 'chosen'}{$t('settings.audio.isolationChosen')}
			{:else}{$t('settings.audio.isolationAll')}{/if}
			{$t('settings.audio.isolationSupportedFormats')}
		</span>
	</Field>

	{#if audio.pluginIsolation === 'chosen' && audio.alwaysIsolated.length > 0}
		<Field
			label={$t('settings.audio.alwaysIsolated')}
			hint={$t('settings.audio.alwaysIsolatedHint')}
		>
			<div class="isolated-list">
				{#each audio.alwaysIsolated as pluginId (pluginId)}
					<span class="isolated-chip">
						{$plugins.find((p) => p.id === pluginId)?.name ?? pluginId}
						<button
							type="button"
							title={$t('settings.audio.removeAlwaysIsolate')}
							on:click={() =>
								(audio.alwaysIsolated = audio.alwaysIsolated.filter((id) => id !== pluginId))}
							>×</button
						>
					</span>
				{/each}
			</div>
		</Field>
	{/if}
</SubSection>
<SubSection title={$t('settings.audio.routingTitle')}>
	<div class="routing-row">
		<div class="routing-label">
			<span class="routing-name">{$t('settings.audio.masterLr')}</span>
			<span class="routing-desc">{$t('settings.audio.masterLrDesc')}</span>
		</div>
		<div class="routing-assign">
			<CustomSelect
				small
				bind:value={masterRouting}
				options={[
					{ value: 'out-1-2', label: $t('settings.audio.outputs12') },
					...(Math.min(audio.outputChannels, realOutputChannels) >= 4
						? [{ value: 'out-3-4', label: $t('settings.audio.outputs34') }]
						: []),
					...(Math.min(audio.outputChannels, realOutputChannels) >= 6
						? [{ value: 'out-5-6', label: $t('settings.audio.outputs56') }]
						: [])
				]}
			/>
		</div>
	</div>

	<div class="routing-row">
		<div class="routing-label">
			<span class="routing-name">{$t('settings.audio.headphones')}</span>
			<span class="routing-desc">{$t('settings.audio.headphonesDesc')}</span>
		</div>
		<div class="routing-assign">
			<CustomSelect
				small
				bind:value={headphoneRouting}
				options={[
					{ value: 'mirror', label: $t('settings.audio.mirrorMaster') },
					...(Math.min(audio.outputChannels, realOutputChannels) >= 4
						? [{ value: 'out-3-4', label: $t('settings.audio.outputs34') }]
						: []),
					...(Math.min(audio.outputChannels, realOutputChannels) >= 6
						? [{ value: 'out-5-6', label: $t('settings.audio.outputs56') }]
						: [])
				]}
			/>
		</div>
	</div>
</SubSection>

<style lang="scss">
	@use '../../variables.scss' as *;

	.field-hint {
		line-height: 1.2;
		font-size: 10px;
		color: rgba(247, 247, 249, 0.28);
	}

	// ── Latency bar ───────────────────────────────────────────────────────────
	.latency-bar {
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.07);
		border-radius: 7px;
		padding: 12px 14px;
		display: grid;
		grid-template-columns: 1fr auto;
		grid-template-rows: auto auto auto;
		gap: 6px 8px;
		align-items: center;
	}

	.latency-label {
		font-size: 11px;
		color: rgba(247, 247, 249, 0.45);
		grid-column: 1;
		grid-row: 1;
	}

	.latency-value {
		font-size: 13px;
		font-weight: 600;
		font-family: $font-mono;
		color: $text-color;
		grid-column: 2;
		grid-row: 1;
		text-align: right;
	}

	.latency-indicator {
		grid-column: 1 / -1;
		grid-row: 2;
		height: 4px;
		background: rgba(255, 255, 255, 0.07);
		border-radius: 2px;
		overflow: hidden;
	}

	.latency-fill {
		height: 100%;
		border-radius: 2px;
		transition:
			width 0.3s,
			background-color 0.3s;

		&.latency-low {
			background: $color-success;
		}
		&.latency-mid {
			background: $color-warning;
		}
		&.latency-high {
			background: $color-error;
		}
	}

	.latency-hint {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.3);
		grid-column: 1 / -1;
		grid-row: 3;
	}

	.xrun-bar {
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.07);
		border-radius: 7px;
		padding: 12px 14px;
		display: grid;
		grid-template-columns: 1fr auto;
		grid-template-rows: auto auto;
		gap: 6px 8px;
		align-items: center;

		&.xrun-warning {
			border-color: rgba(255, 120, 80, 0.35);
			background: rgba(255, 80, 40, 0.06);
		}
	}

	.xrun-label {
		font-size: 11px;
		color: rgba(247, 247, 249, 0.45);
		grid-column: 1;
		grid-row: 1;
	}

	.xrun-value {
		font-size: 13px;
		font-weight: 600;
		font-family: $font-mono;
		color: $text-color;
		grid-column: 2;
		grid-row: 1;
		text-align: right;

		.xrun-warning & {
			color: $color-warning;
		}
	}

	.xrun-hint {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.3);
		grid-column: 1 / -1;
		grid-row: 2;

		.xrun-warning & {
			color: rgba(255, 180, 120, 0.75);
		}
	}

	// ── Routing ───────────────────────────────────────────────────────────────
	.routing-row {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 10px 12px;
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.07);
		border-radius: 6px;
	}

	.routing-label {
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.routing-name {
		font-size: 12px;
		font-weight: 500;
		color: $text-color;
	}

	.routing-desc {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.35);
	}

	.routing-assign {
		flex-shrink: 0;
	}

	// ── Single-value display (e.g. unique sample rate) ───────────────────────
	.single-value-display {
		display: flex;
		align-items: center;
		gap: 6px;
		height: $ctrl-height;
		padding: 0 10px;
		background: $surface-1;
		border: 1px solid $border-faint;
		border-radius: $radius-md;
		font-size: 12px;
		font-weight: 600;
		font-family: $font-mono;
		color: $text-color;
		cursor: default;
	}

	.isolated-list {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}

	.isolated-chip {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 2px 4px 2px 8px;
		font-size: 11px;
		border: 1px solid rgba(34, 197, 94, 0.45);
		background: rgba(34, 197, 94, 0.1);
		border-radius: $radius-xl;
		color: #4ade80;

		button {
			border: none;
			background: none;
			color: inherit;
			cursor: pointer;
			font-size: 13px;
			line-height: 1;
			padding: 0 2px;
			&:hover {
				color: #ff6b61;
			}
		}
	}
</style>
