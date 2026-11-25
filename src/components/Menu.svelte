<script lang="ts">
	import { createEventDispatcher, onMount } from 'svelte';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';

	/** Viewport position of a context menu. Leave both unset to anchor below the parent. */
	export let x: number | undefined = undefined;
	export let y: number | undefined = undefined;
	/** Extra inline style, e.g. a fixed width or custom anchor insets. */
	export let style = '';

	const dispatch = createEventDispatcher<{ close: void }>();
	const viewportMargin = 8;

	$: floating = x !== undefined && y !== undefined;

	let element: HTMLDivElement;
	let left = x ?? 0;
	let top = y ?? 0;

	onMount(() => {
		if (!floating) return;
		const rect = element.getBoundingClientRect();
		if (left + rect.width > window.innerWidth)
			left = window.innerWidth - rect.width - viewportMargin;
		if (top + rect.height > window.innerHeight)
			top = window.innerHeight - rect.height - viewportMargin;
	});

	$: placement = floating ? `left: ${left}px; top: ${top}px;` : '';
</script>

<div
	class="menu"
	class:menu--floating={floating}
	bind:this={element}
	role="menu"
	tabindex="-1"
	use:focusTrap={{ initial: 'first' }}
	use:dismissable={() => dispatch('close')}
	style="{placement} {style}"
>
	<slot />
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.menu {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		z-index: $z-overlay;
		min-width: 200px;
		padding: $space-2;
		background: $surface-overlay;
		border: 1px solid $border-color;
		border-radius: $radius-lg;
		box-shadow: $shadow-overlay;
		user-select: none;

		&--floating {
			position: fixed;
			top: 0;
			left: 0;
		}

		@media (prefers-reduced-motion: no-preference) {
			animation: menu-appear $duration-base $ease-out;
		}
	}

	@keyframes menu-appear {
		from {
			opacity: 0;
			transform: translateY(-3px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}
</style>
