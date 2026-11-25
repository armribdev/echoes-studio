<script lang="ts">
	import { engineProject } from '../frontend/store/engineStore';
	import { updateEffect } from '../frontend/commands/dispatcher';
	import { sidechainSourceCandidates } from '../frontend/routing/sidechain';
	import { notify } from '../stores/status';
	import type { PluginEffectParams } from '../types/global';

	/** Picks the track whose signal feeds a plugin's sidechain input. */
	export let trackId: string;
	export let effectId: string;
	export let params: PluginEffectParams;
	export let disabledReason: string | null = null;

	const NONE = '';

	$: candidates = sidechainSourceCandidates($engineProject.tracks, trackId);
	$: selected = params.sidechain_source ?? NONE;
	$: missing = selected !== NONE && !candidates.some((t) => t.id === selected);

	async function handleChange(event: Event) {
		const value = (event.currentTarget as HTMLSelectElement).value;
		try {
			await updateEffect(trackId, effectId, { sidechain_source: value === NONE ? null : value });
		} catch (e) {
			notify(`Erreur sidechain : ${(e as Error)?.message ?? e}`, 'error');
		}
	}
</script>

<div class="sidechain-row" class:disabled={disabledReason !== null}>
	<label class="sidechain-label" for="sidechain-{effectId}">Sidechain</label>
	<select
		id="sidechain-{effectId}"
		class="sidechain-select"
		value={selected}
		disabled={disabledReason !== null}
		title={disabledReason ??
			'Piste dont le son pilote ce plugin (ex. le kick pour compresser la basse)'}
		on:change={handleChange}
	>
		<option value={NONE}>Aucun</option>
		{#if missing}
			<option value={selected}>Piste supprimée</option>
		{/if}
		{#each candidates as candidate (candidate.id)}
			<option value={candidate.id}>{candidate.name}</option>
		{/each}
	</select>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.sidechain-row {
		display: flex;
		align-items: center;
		gap: $space-3;

		&.disabled {
			opacity: 0.5;
		}
	}

	.sidechain-label {
		flex-shrink: 0;
		font-size: 9px;
		font-family: $font-mono;
		font-weight: 700;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: $text-muted;
	}

	.sidechain-select {
		flex: 1;
		min-width: 0;
		height: $ctrl-height-xs;
		padding-left: $space-4;
		font-size: 10px;
	}
</style>
