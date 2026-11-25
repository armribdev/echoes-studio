<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import CustomSelect from './CustomSelect.svelte';
	import { engineProject } from '../frontend/store/engineStore';
	import { setTrackInput } from '../frontend/commands/dispatcher';
	import { pluginPlacements } from '../frontend/modulation/targets';
	import { instrumentDisplayName, plugins, unavailablePlugins } from '../stores/plugins';
	import { notify } from '../stores/status';
	import type { PluginOutputInfo, PluginOutputSource } from '../types/global';

	/** Takes an aux output of a multi-out plugin (drum machine, sampler) as the track's input. */
	export let trackId: string;

	interface Choice {
		value: string;
		label: string;
		source: PluginOutputSource;
	}

	const NONE = '';

	function valueOf(source: PluginOutputSource): string {
		return `${source.effect_id}#${source.port}`;
	}

	$: track = $engineProject.tracks.find((t) => t.id === trackId);
	$: current = track?.input_source ?? null;

	/** Every plugin this track could listen to: any track's instrument, other tracks' effects (an own effect would loop). */
	$: candidates = $engineProject.tracks.flatMap((t) => {
		const instrumentName = instrumentDisplayName($plugins, t.instrument);
		const effects = t.id === trackId ? [] : t.effects;
		return pluginPlacements(t.id, effects, instrumentName).map((placement) => ({
			...placement,
			trackName: t.name
		}));
	});

	let choices: Choice[] = [];
	let latestRequest = 0;

	async function loadChoices() {
		const request = ++latestRequest;
		const list = candidates;
		const outputs = await Promise.all(
			list.map((placement) =>
				invoke<PluginOutputInfo[]>('get_plugin_outputs', { instanceKey: placement.key }).catch(
					() => [] as PluginOutputInfo[]
				)
			)
		);
		if (request !== latestRequest) return;
		choices = list.flatMap((placement, i) =>
			outputs[i].map((output) => {
				const source = { effect_id: placement.key, port: output.port };
				return {
					value: valueOf(source),
					label: `${placement.trackName} › ${placement.name} › ${output.name || `Sortie ${output.port + 1}`}`,
					source
				};
			})
		);
	}

	function refresh(_placements: string, _recompiled: unknown) {
		void loadChoices();
	}

	// Outputs are known once the plugins are loaded: `unavailablePlugins` is
	// republished after every engine recompile.
	$: placementsKey = candidates.map((c) => c.key).join('|');
	$: refresh(placementsKey, $unavailablePlugins);

	$: selected = current ? valueOf(current) : NONE;
	$: missing = current && !choices.some((c) => c.value === selected);
	$: options = [
		{ value: NONE, label: 'Aucune' },
		...(missing && current ? [{ value: selected, label: 'Sortie indisponible' }] : []),
		...choices.map((choice) => ({ value: choice.value, label: choice.label }))
	];
	$: visible = choices.length > 0 || current !== null;

	async function handleChange(value: string) {
		const source = choices.find((c) => c.value === value)?.source ?? null;
		try {
			await setTrackInput(trackId, source);
		} catch (e) {
			notify(`Erreur entrée : ${(e as Error)?.message ?? e}`, 'error');
		}
	}
</script>

{#if visible}
	<div class="input-row">
		<span class="input-label">Entrée</span>
		<CustomSelect
			size="sm"
			label="Entrée"
			value={selected}
			{options}
			title="Recevoir une sortie séparée d'un plugin multi-sorties (ex. la caisse claire d'une boîte à rythmes)"
			on:open={loadChoices}
			on:change={(e) => handleChange(String(e.detail))}
		/>
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.input-row {
		display: flex;
		align-items: center;
		gap: $space-3;
		padding: $space-2 $panel-inset;
	}

	.input-label {
		flex-shrink: 0;
		font-size: 9px;
		font-family: $font-mono;
		font-weight: 700;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: $text-muted;
	}
</style>
