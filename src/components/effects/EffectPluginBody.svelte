<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import PresetPicker from '../PresetPicker.svelte';
	import RemotePageKnobs from '../RemotePageKnobs.svelte';
	import SidechainPicker from '../SidechainPicker.svelte';
	import type { TrackEffect } from '../../types/global';

	export let trackId: string;
	export let effect: TrackEffect;
	export let format: string | null = null;
	export let compact = false;
	export let missingHelp: string | null = null;
	export let canRetry = false;
	export let retrying = false;
	export let editorOpen = false;
	export let isolatable = false;
	export let alwaysIsolated = false;
	export let chosenIsolated = false;
	export let sandboxed = false;
	export let hasSidechain = false;

	const dispatch = createEventDispatcher<{
		retry: void;
		openEditor: void;
		toggleSandbox: void;
	}>();
</script>

{#if missingHelp !== null}
	<div class="missing-row">
		{#if !compact}
			<p class="missing-help">{missingHelp}</p>
		{/if}
		{#if canRetry}
			<button
				class="retry-btn"
				type="button"
				on:click={() => dispatch('retry')}
				disabled={retrying}
			>
				{retrying ? 'Recherche…' : 'Réessayer'}
			</button>
		{/if}
	</div>
{:else if effect.params.kind === 'plugin'}
	<div class="plugin-editor-row">
		<button
			class="open-editor-btn"
			class:compact
			class:active={editorOpen}
			type="button"
			on:click={() => dispatch('openEditor')}
			title={editorOpen ? "Fermer l'éditeur" : "Ouvrir l'éditeur"}
		>
			{compact
				? editorOpen
					? '× GUI'
					: 'GUI'
				: editorOpen
					? "Fermer l'éditeur"
					: "Ouvrir l'éditeur"}
		</button>
		{#if isolatable}
			<button
				class="open-editor-btn"
				class:compact
				class:active={alwaysIsolated || chosenIsolated}
				type="button"
				disabled={alwaysIsolated}
				on:click={() => dispatch('toggleSandbox')}
				title={alwaysIsolated
					? "Ce plugin est toujours isolé (clic droit sur l'effet pour changer)"
					: "Faire tourner cet effet dans un processus séparé : s'il plante, Echoes continue (clic droit : toujours isoler ce plugin)"}
			>
				{compact ? 'ISO' : alwaysIsolated || chosenIsolated ? 'Isolé' : 'Isoler'}
			</button>
		{/if}
	</div>
	{#if format === 'CLAP' || format === 'VST3'}
		<PresetPicker instanceKey={effect.id} />
	{/if}
	{#if format === 'CLAP'}
		<RemotePageKnobs instanceKey={effect.id} />
	{/if}
	{#if hasSidechain}
		<SidechainPicker
			{trackId}
			effectId={effect.id}
			params={effect.params.params}
			disabledReason={format === 'CLAP' && sandboxed
				? 'Le sidechain ne fonctionne pas quand le plugin CLAP est isolé'
				: null}
		/>
	{/if}
{/if}

<style lang="scss">
	@use '../../variables.scss' as *;

	.missing-row {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 6px;
		padding: 4px 2px;
	}

	.missing-help {
		margin: 0;
		font-size: 9px;
		line-height: 1.35;
		color: $text-subtle;
	}

	.retry-btn {
		padding: 3px 10px;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		background: transparent;
		color: $text-subtle;
		font-size: 10px;
		font-family: $font-mono;
		cursor: pointer;
		&:hover:not(:disabled) {
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;
			border-color: rgba(21, 113, 255, 0.4);
		}
		&:disabled {
			cursor: default;
			opacity: 0.6;
		}
	}

	.plugin-editor-row {
		display: flex;
		justify-content: center;
		gap: 6px;
		padding: 8px 4px;
	}

	.open-editor-btn {
		padding: 5px 14px;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		background: transparent;
		color: $text-subtle;
		font-size: 10px;
		font-family: $font-mono;
		font-weight: 600;
		letter-spacing: 0.04em;
		cursor: pointer;
		transition:
			background $duration-base,
			color $duration-base,
			border-color $duration-base;
		&:hover {
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;
			border-color: rgba(21, 113, 255, 0.4);
		}
		&.active {
			background: rgba(21, 113, 255, 0.2);
			color: $accent-color;
			border-color: rgba(21, 113, 255, 0.5);
		}
	}

	.open-editor-btn.compact {
		padding: 3px 8px;
		font-size: 9px;
	}
</style>
