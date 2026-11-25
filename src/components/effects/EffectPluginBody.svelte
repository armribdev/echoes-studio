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
	@use '../../controls.scss' as *;

	.missing-row {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: $space-3;
	}

	.missing-help {
		margin: 0;
		font-size: 9px;
		line-height: 1.35;
		color: $text-subtle;
	}

	.retry-btn {
		@include chip($ctrl-height-sm);
	}

	.plugin-editor-row {
		display: flex;
		justify-content: center;
		gap: $space-3;
	}

	.open-editor-btn {
		@include chip($ctrl-height-sm);
		padding: 0 $space-6;
		font-weight: 600;

		&.active {
			@include control-on;
		}
	}

	.open-editor-btn.compact {
		height: $ctrl-height-xs;
		padding: 0 $space-4;
	}
</style>
