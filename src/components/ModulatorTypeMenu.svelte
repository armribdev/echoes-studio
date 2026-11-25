<!-- ModulatorTypeMenu.svelte — "+" menu: pick which kind of modulator to add -->
<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { MODULATOR_KINDS } from '../frontend/modulation/modulators';
	import type { ModulatorKind } from '../types/global';
	import Menu from './Menu.svelte';
	import MenuItem from './MenuItem.svelte';

	const dispatch = createEventDispatcher<{ select: ModulatorKind; close: void }>();
</script>

<Menu style="top: calc(100% + 2px); left: 4px; right: 4px; min-width: 0; z-index: 20" on:close>
	{#each MODULATOR_KINDS as info (info.kind)}
		<MenuItem on:click={() => dispatch('select', info.kind)}>
			<svg slot="icon" width="14" height="8" viewBox="-1 -1 16 10" aria-hidden="true"
				><path d={info.icon} /></svg
			>
			<span class="text"
				><span class="title">{info.title}</span><span class="description">{info.description}</span
				></span
			>
		</MenuItem>
	{/each}
</Menu>

<style lang="scss">
	@use '../variables.scss' as *;

	svg {
		color: $accent-color;
	}

	path {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.3;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.text {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.title {
		font-size: 10px;
		font-weight: 600;
	}

	.description {
		font-size: 8px;
		color: $text-faint;
		white-space: normal;
		line-height: 1.3;
	}
</style>
