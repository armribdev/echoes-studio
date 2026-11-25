<!-- TimeRuler.svelte — règle temporelle partagée (arrangement + piano roll) -->
<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { computeRulerTicks } from '../frontend/timeRuler/ticks';
	import { RULER_HEIGHT } from '../frontend/timeRuler/constants';

	export let pxPerBeat: number;
	export let numerator = 4;
	export let scrollPx = 0;
	export let viewportWidthPx: number | undefined = undefined;
	export let maxBars = 2000;

	const dispatch = createEventDispatcher<{ mousedown: MouseEvent }>();

	/** Élément racine — permet au parent de calculer le scrub (getBoundingClientRect). */
	export let rulerEl: HTMLDivElement | undefined = undefined;

	$: ticks = computeRulerTicks({
		pxPerBeat,
		numerator,
		scrollPx,
		viewportWidthPx,
		maxBars
	});

	function tickLeft(x: number): number {
		return x - scrollPx;
	}
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	bind:this={rulerEl}
	class="time-ruler"
	style="height: {RULER_HEIGHT}px"
	on:mousedown={(e) => dispatch('mousedown', e)}
>
	{#each ticks.minor as tick (tick.x)}
		<div class="tick tick--minor" style="left:{tickLeft(tick.x)}px"></div>
	{/each}
	{#each ticks.beat as tick (tick.x)}
		<div class="tick tick--beat" style="left:{tickLeft(tick.x)}px">
			<span class="tick-label tick-label--beat">{tick.label}</span>
		</div>
	{/each}
	{#each ticks.major as tick (tick.x)}
		<div class="tick tick--major" style="left:{tickLeft(tick.x)}px">
			<span class="tick-label">{tick.label}</span>
		</div>
	{/each}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.time-ruler {
		flex-shrink: 0;
		position: relative;
		overflow: hidden;
		background-color: $background-color;
		border-bottom: 1px solid $border-color;
		cursor: pointer;
		user-select: none;
		width: 100%;
	}

	.tick {
		position: absolute;
		bottom: 0;
		width: 1px;

		&--major {
			height: 14px;
			background-color: rgba(247, 247, 249, 0.45);
		}

		&--minor {
			height: 7px;
			background-color: rgba(247, 247, 249, 0.18);
		}

		&--beat {
			height: 7px;
			background-color: rgba(247, 247, 249, 0.18);
		}
	}

	.tick-label {
		position: absolute;
		bottom: 2px;
		left: 4px;
		font-size: 10px;
		line-height: 1;
		color: rgba(247, 247, 249, 0.55);
		white-space: nowrap;
		font-family: $font-mono;
		pointer-events: none;

		&--beat {
			font-size: 9px;
			color: rgba(247, 247, 249, 0.3);
		}
	}
</style>
