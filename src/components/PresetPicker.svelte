<script lang="ts">
	import { unavailablePlugins } from '../stores/plugins';
	import { notify } from '../stores/status';
	import {
		PRESET_GROUP_LABELS,
		activePresetKey,
		fetchPluginPresets,
		groupPresets,
		loadPluginPreset,
		presetErrorMessage,
		presetKey,
		samePresets,
		type PluginPreset
	} from '../frontend/presets/pluginPresets';

	/**
	 * Loads a preset into a plugin. Absent while the plugin offers none. Shows the
	 * preset the plugin says is in use (a VST3 program); empty when it cannot tell.
	 */
	export let instanceKey: string;

	let presets: PluginPreset[] = [];
	let loading = false;
	let select: HTMLSelectElement | undefined;
	let latestRequest = 0;

	$: activeKey = activePresetKey(presets);
	$: unavailable = $unavailablePlugins.has(instanceKey);
	$: refresh(instanceKey, unavailable);

	async function refresh(key: string, isUnavailable: boolean) {
		const request = ++latestRequest;
		const next = isUnavailable ? [] : await fetchPluginPresets(key);
		// An unchanged list leaves the DOM alone, so a menu that is open stays open.
		if (request !== latestRequest || samePresets(next, presets)) return;
		presets = next;
	}

	async function handleChange() {
		const chosen = presets.find((preset) => presetKey(preset) === select?.value);
		if (!chosen) return;
		loading = true;
		try {
			await loadPluginPreset(instanceKey, chosen);
		} catch (e) {
			notify(`Erreur preset : ${presetErrorMessage(e)}`, 'error');
		} finally {
			await refresh(instanceKey, unavailable);
			if (select) select.value = activePresetKey(presets);
			loading = false;
		}
	}
</script>

{#if presets.length > 0}
	<div class="preset-row" class:disabled={loading}>
		<label class="preset-label" for="preset-{instanceKey}">Preset</label>
		<select
			id="preset-{instanceKey}"
			class="preset-select"
			bind:this={select}
			disabled={loading}
			on:focus={() => refresh(instanceKey, unavailable)}
			on:change={handleChange}
		>
			<option value="" hidden selected={activeKey === ''}></option>
			{#each groupPresets(presets) as { group, presets: grouped } (group)}
				<optgroup label={PRESET_GROUP_LABELS[group]}>
					{#each grouped as preset (presetKey(preset))}
						<option value={presetKey(preset)} selected={presetKey(preset) === activeKey}
							>{preset.name}</option
						>
					{/each}
				</optgroup>
			{/each}
		</select>
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.preset-row {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 0 4px 8px;

		&.disabled {
			opacity: 0.5;
		}
	}

	.preset-label {
		flex-shrink: 0;
		font-size: 9px;
		font-family: $font-mono;
		font-weight: 700;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: $text-muted;
	}

	.preset-select {
		flex: 1;
		min-width: 0;
		font-size: 10px;
		padding: 2px 4px;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		background: transparent;
		color: $text-color;
		text-overflow: ellipsis;

		&:disabled {
			cursor: default;
		}

		option,
		optgroup {
			background: #222229;
		}
	}
</style>
