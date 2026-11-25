<!-- MacroCard.svelte — macro body: one value driving every route it feeds -->
<script lang="ts">
	import ModulatorCard from './ModulatorCard.svelte';
	import ParamKnob from './ParamKnob.svelte';
	import {
		formatPercent,
		kindInfo,
		updateModulatorOrNotify
	} from '../frontend/modulation/modulators';
	import type { ProjectModulator } from '../types/global';

	export let trackId: string;
	export let mod: ProjectModulator;
	export let label: string;
	export let color: string;
	export let pickerOpen = false;
	export let routeCount: number;

	let value = mod.value ?? 0;
</script>

<ModulatorCard
	modulatorId={mod.id}
	{label}
	{color}
	icon={kindInfo('macro').icon}
	{pickerOpen}
	on:assign
	on:togglepicker
	on:remove
>
	<div class="macro-row">
		<div class="mod-display meter">
			<div class="fill" style:width="{value * 100}%"></div>
			<span class="big-value">{formatPercent(value)}</span>
			<span class="mod-readout">
				{routeCount === 0 ? 'Aucun réglage' : `${routeCount} réglage${routeCount > 1 ? 's' : ''}`}
			</span>
		</div>
		<ParamKnob
			label="Valeur"
			value={mod.value ?? 0}
			bind:live={value}
			min={0}
			max={1}
			defaultValue={0}
			format={formatPercent}
			{color}
			on:release={(e) => updateModulatorOrNotify(trackId, mod.id, { value: e.detail })}
		/>
	</div>
</ModulatorCard>

<style lang="scss">
	@use '../variables.scss' as *;

	.macro-row {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.meter {
		flex: 1;
		min-width: 0;
	}

	.fill {
		position: absolute;
		inset: 0 auto 0 0;
		background: linear-gradient(
			90deg,
			color-mix(in srgb, var(--mod-color) 10%, transparent),
			color-mix(in srgb, var(--mod-color) 45%, transparent)
		);
		border-right: 2px solid var(--mod-color);
		transition: width 0.05s linear;
	}

	.big-value {
		position: absolute;
		left: 6px;
		bottom: 3px;
		font-size: 14px;
		font-family: $font-mono;
		font-weight: 700;
		color: $text-color;
		text-shadow: $shadow-text;
	}

	.macro-row :global(.param) {
		flex: 0 0 auto;
	}
</style>
