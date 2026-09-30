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
				<!-- svelte-ignore a11y-click-events-have-key-events -->
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

	.cs-wrap {
		position: relative;
		flex: 1;

		&--sm {
			flex: none;
		}
	}

	.cs-trigger {
		width: 100%;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 6px;
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: 5px;
		color: $text-color;
		font-size: 12px;
		font-family: inherit;
		padding: 0 8px;
		height: 30px;
		cursor: pointer;
		text-align: left;
		transition:
			border-color 0.12s,
			background 0.12s;
		white-space: nowrap;

		&:hover {
			border-color: rgba(255, 255, 255, 0.2);
			background: rgba(255, 255, 255, 0.08);
		}

		&--open {
			border-color: $accent-color;
			background: rgba(21, 113, 255, 0.08);
		}
	}

	.cs-value {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.cs-chevron {
		flex-shrink: 0;
		color: rgba(247, 247, 249, 0.4);
		transition: transform 0.15s;

		&--up {
			transform: rotate(180deg);
		}
	}

	.cs-menu {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		min-width: 100%;
		background: #2a2a32;
		border: 1px solid rgba(255, 255, 255, 0.12);
		border-radius: 7px;
		padding: 4px;
		margin: 0;
		list-style: none;
		z-index: 9999;
		box-shadow:
			0 8px 24px rgba(0, 0, 0, 0.6),
			0 0 0 0.5px rgba(255, 255, 255, 0.04);
		overflow: hidden;
	}

	.cs-option {
		display: flex;
		align-items: center;
		gap: 7px;
		padding: 5px 8px;
		border-radius: 5px;
		font-size: 12px;
		color: rgba(247, 247, 249, 0.75);
		cursor: pointer;
		transition:
			background 0.08s,
			color 0.08s;
		white-space: nowrap;

		&:hover {
			background: rgba(255, 255, 255, 0.08);
			color: $text-color;
		}

		&--selected {
			color: $text-color;
			background: rgba(21, 113, 255, 0.12);
		}
	}

	.cs-check {
		flex-shrink: 0;
		color: $accent-color;
	}

	.cs-check-placeholder {
		width: 12px;
		flex-shrink: 0;
	}
</style>
