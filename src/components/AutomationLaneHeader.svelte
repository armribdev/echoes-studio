<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import type { AutomationLane, Track } from '../types/global';
	import { ARRANGE_LANE_HEIGHT } from '../frontend/arrangeInteraction';
	import { laneLabel } from '../frontend/automation/lanes';
	import { ensurePluginParams, pluginParamsByPlacement } from '../stores/pluginParams';
	import { instrumentDisplayName, plugins } from '../stores/plugins';

	export let lane: AutomationLane;
	export let track: Track;
	export let color: string;

	const dispatch = createEventDispatcher<{ remove: void }>();

	$: placementKey = lane.target.kind === 'plugin_param' ? lane.target.effect_id : null;
	$: if (placementKey) ensurePluginParams(placementKey);
	$: label = laneLabel(
		lane.target,
		track.effects,
		instrumentDisplayName($plugins, track.instrument),
		$pluginParamsByPlacement,
	);
</script>

<div class="lane-head" style="height: {ARRANGE_LANE_HEIGHT}px; --lane-color: {color}">
	<div class="labels" title="{label.group} › {label.name}">
		<span class="group">{label.group}</span>
		<span class="name">{label.name}</span>
	</div>
	<button class="remove" type="button" title="Supprimer l'automation" on:click={() => dispatch('remove')}>×</button>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.lane-head {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 0 6px 0 14px;
		box-sizing: border-box;
		border-bottom: 1px solid $border-color;
		position: relative;

		&::before {
			content: '';
			position: absolute;
			left: 6px;
			top: 12px;
			bottom: 12px;
			width: 2px;
			border-radius: 1px;
			background: var(--lane-color);
			opacity: 0.6;
		}
	}

	.labels {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.group,
	.name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.group {
		font-size: 9px;
		font-family: $font-mono;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: $text-faint;
	}

	.name {
		font-size: 11px;
		color: $text-subtle;
	}

	.remove {
		flex-shrink: 0;
		width: 18px;
		height: 18px;
		border: none;
		border-radius: $radius-sm;
		background: transparent;
		color: $text-faint;
		font-size: 13px;
		line-height: 1;
		cursor: pointer;

		&:hover {
			background: $surface-hover;
			color: $text-color;
		}
	}
</style>
