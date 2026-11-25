<script lang="ts">
	import { onMount, onDestroy, createEventDispatcher } from 'svelte';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';

	const dispatch = createEventDispatcher<{ change: string | number }>();

	export let value: string | number;
	export let options: { value: string | number; label: string }[];
	export let small = false;

	let open = false;

	$: selectedLabel = options.find((o) => o.value === value)?.label ?? String(value);

	function toggle() {
		open = !open;
	}

	function select(v: string | number) {
		value = v;
		open = false;
		dispatch('change', v);
	}

	function handleKeydown(e: KeyboardEvent) {
		if (!open) return;
		const idx = options.findIndex((o) => o.value === value);
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			value = options[Math.min(idx + 1, options.length - 1)].value;
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			value = options[Math.max(idx - 1, 0)].value;
		} else if (e.key === 'Enter') {
			open = false;
		}
	}

	onMount(() => {
		document.addEventListener('keydown', handleKeydown);
	});
	onDestroy(() => {
		document.removeEventListener('keydown', handleKeydown);
	});
</script>

<div class="cs-wrap" class:cs-wrap--sm={small}>
	<button
		class="cs-trigger"
		class:cs-trigger--open={open}
		on:click={toggle}
		type="button"
		aria-haspopup="listbox"
		aria-expanded={open}
	>
		<span class="cs-value">{selectedLabel}</span>
		<svg
			class="cs-chevron"
			class:cs-chevron--up={open}
			width="10"
			height="6"
			viewBox="0 0 10 6"
			fill="none"
			xmlns="http://www.w3.org/2000/svg"
		>
			<path
				d="M1 1l4 4 4-4"
				stroke="currentColor"
				stroke-width="1.5"
				stroke-linecap="round"
				stroke-linejoin="round"
			/>
		</svg>
	</button>

	{#if open}
		<ul
			class="cs-menu"
			role="listbox"
			tabindex="-1"
			use:focusTrap={{ initial: 'first' }}
			use:dismissable={() => (open = false)}
		>
			{#each options as opt}
				<li
					class="cs-option"
					class:cs-option--selected={opt.value === value}
					role="option"
					tabindex="0"
					aria-selected={opt.value === value}
					on:click={() => select(opt.value)}
					on:keydown={(e) => {
						if (e.key === 'Enter' || e.key === ' ') {
							e.preventDefault();
							select(opt.value);
						}
					}}
				>
					{#if opt.value === value}
						<svg
							class="cs-check"
							width="12"
							height="12"
							viewBox="0 0 12 12"
							fill="none"
							xmlns="http://www.w3.org/2000/svg"
						>
							<path
								d="M2 6l3 3 5-5"
								stroke="currentColor"
								stroke-width="1.6"
								stroke-linecap="round"
								stroke-linejoin="round"
							/>
						</svg>
					{:else}
						<span class="cs-check-placeholder"></span>
					{/if}
					{opt.label}
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.cs-wrap {
		position: relative;
		flex: 1;

		&--sm {
			flex: none;
		}
	}

	.cs-trigger {
		@include field;
		width: 100%;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 6px;
		padding: 0 8px 0 10px;
		color: $text-strong;
		text-align: left;
		white-space: nowrap;
		cursor: pointer;

		&:not(:disabled):hover {
			background: $surface-3;
		}

		&--open {
			border-color: $accent-color;
			background: $accent-wash;
		}
	}

	.cs-value {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.cs-chevron {
		flex-shrink: 0;
		color: $text-subtle;
		transition: transform $duration-base;

		&--up {
			transform: rotate(180deg);
			color: $accent-text;
		}
	}

	.cs-menu {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		min-width: 100%;
		background: $surface-overlay;
		border: 1px solid $border-subtle;
		border-radius: $radius-lg;
		padding: 4px;
		margin: 0;
		list-style: none;
		z-index: 9999;
		box-shadow: $shadow-overlay;
		overflow: hidden;
	}

	.cs-option {
		@include list-item;
		white-space: nowrap;

		&--selected {
			@include list-item-selected;
		}
	}

	.cs-check {
		flex-shrink: 0;
		color: $accent-text;
	}

	.cs-check-placeholder {
		width: 12px;
		flex-shrink: 0;
	}
</style>
