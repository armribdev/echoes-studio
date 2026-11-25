<script lang="ts">
	import { listen } from '@tauri-apps/api/event';
	import { onMount } from 'svelte';
	import CustomSelect from './CustomSelect.svelte';
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

	const EDIT_REFRESH_DELAY_MS = 300;

	let presets: PluginPreset[] = [];
	let loading = false;
	let shownKey = '';
	let latestRequest = 0;
	let refreshTimer: ReturnType<typeof setTimeout> | undefined;

	// A plugin window edits without telling the picker; wait for a gesture to end before listing.
	onMount(() => {
		const unlisten = listen('plugin-state-edited', () => {
			clearTimeout(refreshTimer);
			refreshTimer = setTimeout(() => {
				if (!loading) refresh(instanceKey, unavailable);
			}, EDIT_REFRESH_DELAY_MS);
		});
		return () => {
			clearTimeout(refreshTimer);
			unlisten.then((stop) => stop());
		};
	});

	$: activeKey = activePresetKey(presets);
	$: shownKey = activeKey;
	$: options = groupPresets(presets).flatMap(({ group, presets: grouped }) =>
		grouped.map((preset) => ({
			value: presetKey(preset),
			label: preset.name,
			group: PRESET_GROUP_LABELS[group]
		}))
	);
	$: unavailable = $unavailablePlugins.has(instanceKey);
	$: refresh(instanceKey, unavailable);

	async function refresh(key: string, isUnavailable: boolean) {
		const request = ++latestRequest;
		const next = isUnavailable ? [] : await fetchPluginPresets(key);
		// An unchanged list leaves the DOM alone, so a menu that is open stays open.
		if (request !== latestRequest || samePresets(next, presets)) return;
		presets = next;
	}

	async function handleChange(key: string) {
		const chosen = presets.find((preset) => presetKey(preset) === key);
		if (!chosen) return;
		loading = true;
		try {
			await loadPluginPreset(instanceKey, chosen);
		} catch (e) {
			notify(`Erreur preset : ${presetErrorMessage(e)}`, 'error');
		} finally {
			await refresh(instanceKey, unavailable);
			shownKey = activePresetKey(presets);
			loading = false;
		}
	}
</script>

{#if presets.length > 0}
	<div class="preset-row" class:disabled={loading}>
		<span class="preset-label">Preset</span>
		<CustomSelect
			size="sm"
			label="Preset"
			value={shownKey}
			{options}
			disabled={loading}
			on:open={() => refresh(instanceKey, unavailable)}
			on:change={(e) => handleChange(String(e.detail))}
		/>
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.preset-row {
		display: flex;
		align-items: center;
		gap: $space-3;

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
</style>
