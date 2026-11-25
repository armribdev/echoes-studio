<script lang="ts">
	import { t } from 'svelte-i18n';
	import CustomSelect from '../CustomSelect.svelte';
	import Icon from '../Icon.svelte';
	import type { Preferences } from '../../stores/preferences';

	export let audio: Preferences['audio'];
	export let inputDeviceOptions: { value: string; label: string }[];
	export let outputDeviceOptions: { value: string; label: string }[];
	export let devicesLoading: boolean;
	export let channelsLoading: boolean;
	export let realInputChannels: number;
	export let realOutputChannels: number;
	export let displayInputLevels: number[];
	export let displayOutputLevels: number[];
	export let applyError: string | null;

	/**
	 * Map a linear amplitude peak to a CSS height percentage using a dB scale.
	 * −60 dB → 0 %   |   0 dB → 100 %   |   above 0 dB is clamped to 100 %.
	 */
	function levelToHeight(amp: number): string {
		if (amp <= 0) return '0%';
		const db = 20 * Math.log10(amp);
		const pct = Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
		return `${pct.toFixed(1)}%`;
	}
</script>

<div class="io-grid">
	<!-- Colonne Entrée -->
	<div class="io-col">
		<div class="io-device-row">
			<span class="io-device-label">
				{$t('settings.audio.input')}
				<span class="badge badge--in">IN</span>
				{#if devicesLoading}<span class="loading-dot">…</span>{/if}
			</span>
			<CustomSelect bind:value={audio.inputDevice} options={inputDeviceOptions} />
			<span class="field-hint">{$t('settings.audio.inputHint')}</span>
		</div>

		<div class="channel-card channel-card--in">
			<div class="channel-card-header">
				<span class="channel-card-title">{$t('settings.audio.activeChannels')}</span>
				<div class="channel-count-control">
					<button
						class="count-btn"
						disabled={audio.inputChannels <= 1}
						on:click={() => (audio.inputChannels = Math.max(1, audio.inputChannels - 1))}>−</button
					>
					<span class="count-value">{Math.min(audio.inputChannels, realInputChannels)}</span>
					<button
						class="count-btn"
						disabled={audio.inputChannels >= realInputChannels}
						on:click={() =>
							(audio.inputChannels = Math.min(realInputChannels, audio.inputChannels + 1))}
						>+</button
					>
				</div>
			</div>
			<div class="channel-strips">
				{#each Array(Math.min(realInputChannels, 16)) as _, i}
					<div
						class="channel-strip channel-strip--in"
						class:channel-strip--inactive={i >= Math.min(audio.inputChannels, realInputChannels)}
						title={$t('settings.audio.inputChannel', { values: { n: i + 1 } })}
					>
						<span class="strip-num">{i + 1}</span>
						<div class="strip-meter">
							<div
								class="strip-level strip-level--in"
								style="height: {levelToHeight(displayInputLevels[i] ?? 0)}"
							></div>
						</div>
					</div>
				{/each}
			</div>
		</div>
	</div>

	<!-- Flèche centrale -->
	<div class="channel-route-arrow">
		<Icon name="arrow-right" size={14} />
	</div>

	<!-- Colonne Sortie -->
	<div class="io-col">
		<div class="io-device-row">
			<span class="io-device-label">
				{$t('settings.audio.output')}
				<span class="badge badge--out">OUT</span>
				{#if devicesLoading}<span class="loading-dot">…</span>{/if}
			</span>
			<CustomSelect bind:value={audio.outputDevice} options={outputDeviceOptions} />
			<span class="field-hint">{$t('settings.audio.outputHint')}</span>
		</div>

		<div class="channel-card channel-card--out">
			<div class="channel-card-header">
				<span class="channel-card-title">{$t('settings.audio.activeChannels')}</span>
				{#if channelsLoading}<span class="loading-dot">…</span>{/if}
				<div class="channel-count-control">
					<button
						class="count-btn"
						disabled={audio.outputChannels <= 1}
						on:click={() => (audio.outputChannels = Math.max(1, audio.outputChannels - 1))}
						>−</button
					>
					<span class="count-value">{Math.min(audio.outputChannels, realOutputChannels)}</span>
					<button
						class="count-btn"
						disabled={audio.outputChannels >= realOutputChannels}
						on:click={() =>
							(audio.outputChannels = Math.min(realOutputChannels, audio.outputChannels + 1))}
						>+</button
					>
				</div>
			</div>
			<div class="channel-strips">
				{#each Array(Math.min(realOutputChannels, 16)) as _, i}
					<div
						class="channel-strip channel-strip--out"
						class:channel-strip--inactive={i >= Math.min(audio.outputChannels, realOutputChannels)}
						title={$t('settings.audio.outputChannel', { values: { n: i + 1 } })}
					>
						<span class="strip-num">{i + 1}</span>
						<div class="strip-meter">
							<div
								class="strip-level strip-level--out"
								style="height: {levelToHeight(displayOutputLevels[i] ?? 0)}"
							></div>
						</div>
					</div>
				{/each}
			</div>
		</div>
	</div>
</div>
<!-- /io-grid -->

{#if applyError}
	<div class="apply-error">
		<Icon name="alert-triangle" size={13} />
		<span>{applyError}</span>
	</div>
{/if}

<style lang="scss">
	@use '../../variables.scss' as *;

	.field-hint {
		line-height: 1.2;
		font-size: 10px;
		color: rgba(247, 247, 249, 0.28);
	}

	// ── Badge ─────────────────────────────────────────────────────────────────
	.badge {
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.8px;
		padding: 2px 5px;
		border-radius: $radius-xs;

		&--in {
			background: rgba(0, 200, 120, 0.15);
			color: $color-success;
			border: 1px solid rgba(0, 200, 120, 0.3);
		}

		&--out {
			background: rgba(21, 113, 255, 0.15);
			color: #5599ff;
			border: 1px solid rgba(21, 113, 255, 0.3);
		}
	}

	// ── IO two-column grid ────────────────────────────────────────────────────
	.io-grid {
		display: grid;
		grid-template-columns: 1fr auto 1fr;
		gap: 10px;
		align-items: start;
	}

	.io-col {
		display: flex;
		flex-direction: column;
		gap: 8px;
		min-width: 0;
	}

	.io-device-row {
		display: flex;
		flex-direction: column;
		gap: 5px;
	}

	.io-device-label {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 11px;
		color: rgba(247, 247, 249, 0.55);
		font-weight: 500;
		letter-spacing: 0.3px;
	}

	// ── Channel cards ─────────────────────────────────────────────────────────
	.channel-card {
		background: $surface-1;
		border: 1px solid $border-faint;
		border-radius: $radius-lg;
		padding: 12px;
		display: flex;
		flex-direction: column;
		gap: 10px;

		&--in {
			border-top: 2px solid rgba(0, 200, 120, 0.35);
		}
		&--out {
			border-top: 2px solid rgba(21, 113, 255, 0.35);
		}
	}

	.channel-card-header {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.channel-card-title {
		font-size: 11px;
		font-weight: 500;
		color: rgba(247, 247, 249, 0.45);
	}

	.channel-count-control {
		display: flex;
		gap: 8px;
	}

	.count-btn {
		width: 20px;
		height: 20px;
		border: 1px solid rgba(255, 255, 255, 0.12);
		background: $surface-2;
		border-radius: $radius-md;
		color: $text-color;
		font-size: 12px;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		transition:
			background $duration-base,
			border-color $duration-base;

		&:hover:not(:disabled) {
			background: rgba(255, 255, 255, 0.1);
			border-color: rgba(255, 255, 255, 0.2);
		}

		&:disabled {
			opacity: 0.3;
			cursor: default;
		}
	}

	.count-value {
		font-size: 16px;
		font-weight: 600;
		font-family: $font-mono;
		color: $text-color;
		min-width: 22px;
		text-align: center;
	}

	.channel-route-arrow {
		display: flex;
		align-items: center;
		color: rgba(247, 247, 249, 0.2);
		flex-shrink: 0;
		align-self: center;
	}

	.channel-strips {
		display: flex;
		gap: 3px;
		align-items: flex-end;
		flex-wrap: wrap;
	}

	.channel-strip {
		width: 16px;
		border-radius: 3px 3px 0 0;
		overflow: hidden;
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.08);
		display: flex;
		flex-direction: column;
		align-items: center;
		padding: 2px 0 2px;
		gap: 2px;
		height: 36px;

		&--in {
			border-bottom: 2px solid rgba(0, 200, 120, 0.5);
		}
		&--out {
			border-bottom: 2px solid rgba(21, 113, 255, 0.5);
		}

		&--inactive {
			opacity: 0.4;
			filter: grayscale(1);
		}
	}

	.strip-num {
		font-size: 7px;
		color: rgba(247, 247, 249, 0.3);
		font-family: ui-monospace, monospace;
		line-height: 1;
	}

	.strip-meter {
		flex: 1;
		width: 6px;
		background: rgba(255, 255, 255, 0.05);
		border-radius: 1px;
		display: flex;
		align-items: flex-end;
	}

	.strip-level {
		width: 100%;
		border-radius: 1px;

		&--in {
			background: rgba(0, 200, 120, 0.65);
		}
		&--out {
			background: rgba(21, 113, 255, 0.65);
		}
	}

	// ── Device loading / error states ────────────────────────────────────────
	.loading-dot {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.3);
		margin-left: 2px;
	}

	.apply-error {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 8px 10px;
		background: rgba(255, 60, 60, 0.08);
		border: 1px solid rgba(255, 60, 60, 0.25);
		border-radius: 6px;
		color: #ff7070;
		font-size: 11px;
	}
</style>
