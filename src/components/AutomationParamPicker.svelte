<script lang="ts">
	import { createEventDispatcher, onMount } from 'svelte';
	import ModulationTargetPicker from './ModulationTargetPicker.svelte';
	import type { Track } from '../types/global';
	import {
		buildAutomationOptions,
		pluginPlacements,
		type ModulationTargetOption
	} from '../frontend/modulation/targets';
	import { targetKey } from '../frontend/automation/lanes';
	import { pluginParamsByPlacement, reloadPluginParams } from '../stores/pluginParams';
	import { instrumentDisplayName, plugins } from '../stores/plugins';

	export let track: Track;
	export let x: number;
	export let y: number;

	const WIDTH_PX = 260;
	const MAX_HEIGHT_PX = 290;

	const dispatch = createEventDispatcher<{ select: ModulationTargetOption; close: void }>();

	let loading = true;

	$: placements = pluginPlacements(
		track.id,
		track.effects,
		instrumentDisplayName($plugins, track.instrument)
	);
	$: automated = new Set(track.automation_lanes.map((lane) => targetKey(lane.target)));
	$: options = buildAutomationOptions(track.effects, placements, $pluginParamsByPlacement).filter(
		(option) => !automated.has(option.key)
	);
	$: left = Math.max(8, Math.min(x, window.innerWidth - WIDTH_PX - 8));
	$: top = Math.max(8, Math.min(y, window.innerHeight - MAX_HEIGHT_PX - 8));

	onMount(async () => {
		await Promise.all(placements.map((placement) => reloadPluginParams(placement.key)));
		loading = false;
	});
</script>

<div class="automation-picker" style="left: {left}px; top: {top}px; width: {WIDTH_PX}px">
	<ModulationTargetPicker
		{options}
		{loading}
		on:select={(e) => dispatch('select', e.detail)}
		on:close={() => dispatch('close')}
	/>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.automation-picker {
		position: fixed;
		z-index: 201;
		box-shadow: $shadow-overlay;

		:global(.picker) {
			margin: 0;
		}
	}
</style>
