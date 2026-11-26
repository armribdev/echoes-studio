<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import type { AutomationLane, Track } from '../types/global';
	import { ARRANGE_LANE_HEIGHT } from '../frontend/arrangeInteraction';
	import { laneLabel, type AutomationShape } from '../frontend/automation/lanes';
	import { ensurePluginParams, pluginParamsByPlacement } from '../stores/pluginParams';
	import { instrumentDisplayName, plugins } from '../stores/plugins';
	import { loopRegion } from '../stores/loopRegion';
	import { toggleLaneWrite, writeArmedLanes } from '../stores/automationWrite';
	import Menu from './Menu.svelte';
	import MenuItem from './MenuItem.svelte';

	export let lane: AutomationLane;
	export let track: Track;
	export let color: string;

	const dispatch = createEventDispatcher<{
		remove: void;
		toggleStep: void;
		shape: AutomationShape;
	}>();

	const SHAPES: { shape: AutomationShape; label: string }[] = [
		{ shape: 'ramp_up', label: 'Rampe montante' },
		{ shape: 'ramp_down', label: 'Rampe descendante' },
		{ shape: 'sine', label: 'Sinus' }
	];

	let shapeMenu: { x: number; y: number } | null = null;

	function openShapeMenu(e: MouseEvent) {
		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		shapeMenu = { x: rect.left, y: rect.bottom + 4 };
	}

	function pickShape(shape: AutomationShape) {
		shapeMenu = null;
		dispatch('shape', shape);
	}

	$: placementKey = lane.target.kind === 'plugin_param' ? lane.target.effect_id : null;
	$: if (placementKey) ensurePluginParams(placementKey);
	$: label = laneLabel(
		lane.target,
		track.effects,
		instrumentDisplayName($plugins, track.instrument),
		$pluginParamsByPlacement
	);
</script>

<div class="lane-head" style="height: {ARRANGE_LANE_HEIGHT}px; --lane-color: {color}">
	<div class="labels" title="{label.group} › {label.name}">
		<span class="group">{label.group}</span>
		<span class="name">{label.name}</span>
	</div>
	<button
		class="interpolation"
		class:on={lane.interpolation === 'step'}
		type="button"
		title={lane.interpolation === 'step'
			? 'Escalier (cliquer pour linéaire)'
			: 'Linéaire (cliquer pour escalier)'}
		aria-pressed={lane.interpolation === 'step'}
		on:click={() => dispatch('toggleStep')}
	>
		<svg
			width="12"
			height="12"
			viewBox="0 0 12 12"
			fill="none"
			stroke="currentColor"
			stroke-width="1.5"
		>
			{#if lane.interpolation === 'step'}
				<path d="M1 10 H4 V6 H8 V2 H11" />
			{:else}
				<path d="M1 10 L11 2" />
			{/if}
		</svg>
	</button>
	<button
		class="shapes"
		class:on={shapeMenu !== null}
		type="button"
		title={$loopRegion.enabled
			? 'Remplir la boucle avec une forme'
			: 'Active la boucle pour choisir la plage à remplir'}
		aria-haspopup="menu"
		aria-expanded={shapeMenu !== null}
		disabled={!$loopRegion.enabled}
		on:click={openShapeMenu}
	>
		<svg
			width="12"
			height="12"
			viewBox="0 0 12 12"
			fill="none"
			stroke="currentColor"
			stroke-width="1.5"
		>
			<path d="M1 6 C3 0, 5 0, 6 6 S9 12, 11 6" />
		</svg>
	</button>
	{#if lane.target.kind === 'plugin_param'}
		<button
			class="write"
			class:on={$writeArmedLanes.has(lane.id)}
			type="button"
			title="Écrire en lecture : un geste dans la fenêtre du plugin remplace les points"
			aria-pressed={$writeArmedLanes.has(lane.id)}
			on:click={() => toggleLaneWrite(lane.id)}
		>
			<svg width="12" height="12" viewBox="0 0 12 12" fill="currentColor">
				<circle cx="6" cy="6" r="3.5" />
			</svg>
		</button>
	{/if}
	<button
		class="remove"
		type="button"
		title="Supprimer l'automation"
		on:click={() => dispatch('remove')}>×</button
	>
</div>

{#if shapeMenu}
	<Menu x={shapeMenu.x} y={shapeMenu.y} on:close={() => (shapeMenu = null)}>
		{#each SHAPES as { shape, label }}
			<MenuItem on:click={() => pickShape(shape)}>{label}</MenuItem>
		{/each}
	</Menu>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

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

	.interpolation {
		@include icon-button($ctrl-height-xs);

		&.on {
			@include control-on;
		}
	}

	.write,
	.shapes {
		@include icon-button($ctrl-height-xs);

		&.on {
			@include control-on;
		}
	}

	.remove {
		@include icon-button($ctrl-height-xs);
		font-size: 13px;
	}
</style>
