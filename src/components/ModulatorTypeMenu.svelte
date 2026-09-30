<!-- ModulatorTypeMenu.svelte — "+" menu: pick which kind of modulator to add -->
<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { MODULATOR_KINDS } from '../frontend/modulation/modulators';
	import type { ModulatorKind } from '../types/global';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';

	const dispatch = createEventDispatcher<{ select: ModulatorKind; close: void }>();

	let root: HTMLDivElement;
</script>

<div
	class="menu"
	role="menu"
	tabindex="-1"
	bind:this={root}
	use:focusTrap={{ initial: 'first' }}
	use:dismissable={() => dispatch('close')}
>
	{#each MODULATOR_KINDS as info (info.kind)}
		<button
			class="item"
			type="button"
			role="menuitem"
			on:click={() => dispatch('select', info.kind)}
		>
			<svg width="14" height="8" viewBox="-1 -1 16 10" aria-hidden="true"
				><path d={info.icon} /></svg
			>
			<span class="text"
				><span class="title">{info.title}</span><span class="description">{info.description}</span
				></span
			>
		</button>
	{/each}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.menu {
		position: absolute;
		top: calc(100% + 2px);
		left: 4px;
		right: 4px;
		z-index: 20;
		display: flex;
		flex-direction: column;
		padding: 3px;
		border: 1px solid $border-subtle;
		border-radius: 5px;
		background: #1e1e26;
		box-shadow: 0 6px 18px rgba(0, 0, 0, 0.55);
		animation: appear 0.12s ease-out;
	}

	@keyframes appear {
		from {
			opacity: 0;
			transform: translateY(-3px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	.item {
		display: flex;
		align-items: center;
		gap: 7px;
		padding: 5px 6px;
		border: none;
		border-radius: 3px;
		background: transparent;
		color: $text-subtle;
		text-align: left;
		cursor: pointer;
		&:hover,
		&:focus-visible {
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;
			outline: none;
		}
	}

	svg {
		flex-shrink: 0;
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
