<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';
	import { appInert, portal } from '../frontend/portal';

	/** Accessible name of the dialog. */
	export let label: string;
	export let width = 440;
	/** Fixed height in px; the dialog sizes to its content when unset. */
	export let height: number | undefined = undefined;
	export let initialFocus: 'container' | 'first' = 'container';

	const dispatch = createEventDispatcher<{ close: void }>();
</script>

<div class="backdrop" role="presentation" use:portal use:appInert>
	<div
		class="dialog"
		role="dialog"
		aria-modal="true"
		aria-label={label}
		tabindex="-1"
		use:focusTrap={{ initial: initialFocus }}
		use:dismissable={() => dispatch('close')}
		style:width="{width}px"
		style:height={height === undefined ? undefined : `${height}px`}
	>
		<slot />
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.backdrop {
		position: fixed;
		inset: 0;
		z-index: $z-modal;
		display: flex;
		align-items: center;
		justify-content: center;
		background: $scrim;
		backdrop-filter: blur(4px);

		@media (prefers-reduced-motion: no-preference) {
			animation: backdrop-appear $duration-base $ease-out;
		}
	}

	.dialog {
		display: flex;
		flex-direction: column;
		max-width: calc(100vw - 48px);
		max-height: calc(100vh - 80px);
		overflow: hidden;
		background: $surface-dialog;
		border: 1px solid $border-color;
		border-radius: $radius-xl;
		box-shadow: $shadow-dialog;

		@media (prefers-reduced-motion: no-preference) {
			animation: dialog-appear $duration-base $ease-out;
		}
	}

	@keyframes backdrop-appear {
		from {
			opacity: 0;
		}
	}

	@keyframes dialog-appear {
		from {
			opacity: 0;
			transform: translateY(6px) scale(0.98);
		}
	}
</style>
