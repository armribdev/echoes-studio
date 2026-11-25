<script lang="ts">
	import type { Track } from '../types/global';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';
	import { addAutomationLane, removeAutomationLane } from '../frontend/commands/dispatcher';
	import {
		buildAutomationOptions,
		pluginPlacements,
		type ModulationTargetOption
	} from '../frontend/modulation/targets';
	import { targetKey } from '../frontend/automation/lanes';
	import { automationRequest, type AutomationRequest } from '../stores/automation';
	import { pluginParamsByPlacement } from '../stores/pluginParams';
	import { instrumentDisplayName, plugins } from '../stores/plugins';
	import { notify } from '../stores/status';
	import { t } from 'svelte-i18n';

	export let track: Track | undefined;

	const MENU_WIDTH_PX = 230;

	let menu: { x: number; y: number; option: ModulationTargetOption; laneId: string | null } | null =
		null;

	$: if ($automationRequest) show($automationRequest);

	function show(request: AutomationRequest) {
		automationRequest.set(null);
		if (!track) return;
		const placements = pluginPlacements(
			track.id,
			track.effects,
			instrumentDisplayName($plugins, track.instrument)
		);
		const option = buildAutomationOptions(track.effects, placements, $pluginParamsByPlacement).find(
			(o) => o.key === request.key
		);
		if (!option) return;
		const lane = track.automation_lanes.find((l) => targetKey(l.target) === request.key);
		menu = {
			x: Math.min(request.x, window.innerWidth - MENU_WIDTH_PX - 8),
			y: request.y,
			option,
			laneId: lane?.id ?? null
		};
	}

	function fail(error: unknown) {
		notify(`Erreur automation : ${(error as Error)?.message ?? error}`, 'error');
	}

	function automate() {
		if (!track || !menu) return;
		const { option } = menu;
		menu = null;
		addAutomationLane(track.id, option.kind, option.param, option.effectId).catch(fail);
	}

	function removeLane() {
		if (!track || !menu?.laneId) return;
		const laneId = menu.laneId;
		menu = null;
		removeAutomationLane(track.id, laneId).catch(fail);
	}
</script>

{#if menu}
	<div
		class="context-menu"
		role="menu"
		tabindex="-1"
		use:focusTrap={{ initial: 'first' }}
		use:dismissable={() => (menu = null)}
		style="left: {menu.x}px; top: {menu.y}px; width: {MENU_WIDTH_PX}px"
	>
		<div class="menu-header">{menu.option.group} › {menu.option.label}</div>
		{#if menu.laneId}
			<button class="item item--danger" role="menuitem" type="button" on:click={removeLane}
				>{$t('automationMenu.remove')}</button
			>
		{:else}
			<button class="item" role="menuitem" type="button" on:click={automate}
				>{$t('automationMenu.automate')}</button
			>
		{/if}
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.context-menu {
		position: fixed;
		z-index: 201;
		box-sizing: border-box;
		background: $surface-overlay;
		border: 1px solid $border-color;
		border-radius: $radius-lg;
		padding: 4px;
		box-shadow: $shadow-overlay;
		user-select: none;
	}

	.menu-header {
		padding: 4px 10px 6px;
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.6px;
		text-transform: uppercase;
		color: $text-faint;
		font-family: $font-mono;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.item {
		@include list-item;

		&--danger {
			color: $color-error;

			&:hover {
				background: $error-soft;
				color: $color-error;
			}
		}
	}
</style>
