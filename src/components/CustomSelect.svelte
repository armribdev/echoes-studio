<script lang="ts">
	import { createEventDispatcher, tick } from 'svelte';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';

	interface SelectOption {
		value: string | number;
		label: string;
		/** Consecutive options sharing a group are listed under its heading. */
		group?: string;
	}

	const MENU_GAP = 4;
	const VIEWPORT_MARGIN = 8;
	const MAX_MENU_HEIGHT = 320;
	const MIN_ROOM_BELOW = 160;

	const dispatch = createEventDispatcher<{ change: string | number; open: void }>();

	export let value: string | number;
	export let options: SelectOption[];
	export let small = false;
	export let size: 'xs' | 'sm' | 'md' = 'md';
	export let disabled = false;
	export let title: string | undefined = undefined;
	export let label: string | undefined = undefined;
	/** Shown while `value` matches no option. */
	export let placeholder = '';

	let open = false;
	let trigger: HTMLButtonElement;
	let menu: HTMLUListElement | undefined;
	let menuStyle = '';

	$: selectedLabel =
		options.find((o) => o.value === value)?.label ?? (placeholder || String(value));
	$: rows = options.map((option, index) => ({
		option,
		heading: option.group && option.group !== options[index - 1]?.group ? option.group : undefined
	}));

	function place() {
		const rect = trigger.getBoundingClientRect();
		const below = window.innerHeight - rect.bottom - VIEWPORT_MARGIN;
		const above = rect.top - VIEWPORT_MARGIN;
		const upward = below < MIN_ROOM_BELOW && above > below;
		const room = Math.min(MAX_MENU_HEIGHT, upward ? above : below) - MENU_GAP;
		const anchor = upward
			? `bottom: ${window.innerHeight - rect.top + MENU_GAP}px;`
			: `top: ${rect.bottom + MENU_GAP}px;`;
		menuStyle = `left: ${rect.left}px; min-width: ${rect.width}px; max-height: ${room}px; ${anchor}`;
	}

	async function openMenu() {
		place();
		open = true;
		dispatch('open');
		await tick();
		const target = menu?.querySelector<HTMLElement>('[aria-selected="true"]');
		target?.focus();
	}

	function toggle() {
		if (open) open = false;
		else void openMenu();
	}

	function select(v: string | number) {
		value = v;
		open = false;
		trigger.focus();
		dispatch('change', v);
	}

	function closeOnOutsideScroll(e: Event) {
		if (open && !menu?.contains(e.target as Node)) open = false;
	}

	function handleOptionKeydown(e: KeyboardEvent, v: string | number) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			select(v);
			return;
		}
		const step = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0;
		if (step === 0 || !menu) return;
		e.preventDefault();
		const items = [...menu.querySelectorAll<HTMLElement>('[role="option"]')];
		const next = items.indexOf(e.currentTarget as HTMLElement) + step;
		items[Math.max(0, Math.min(items.length - 1, next))].focus();
	}
</script>

<svelte:window on:resize={() => (open = false)} on:scroll|capture={closeOnOutsideScroll} />

<div class="cs-wrap" class:cs-wrap--sm={small}>
	<button
		bind:this={trigger}
		class="cs-trigger cs-trigger--{size}"
		class:cs-trigger--open={open}
		on:click={toggle}
		type="button"
		{disabled}
		{title}
		aria-label={label}
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
			bind:this={menu}
			class="cs-menu"
			role="listbox"
			tabindex="-1"
			aria-label={label}
			style={menuStyle}
			use:focusTrap={{ initial: 'first' }}
			use:dismissable={() => (open = false)}
		>
			{#each rows as { option, heading } (option.value)}
				{#if heading}
					<li class="cs-group" role="presentation">{heading}</li>
				{/if}
				<li
					class="cs-option"
					class:cs-option--selected={option.value === value}
					role="option"
					tabindex="0"
					aria-selected={option.value === value}
					on:click={() => select(option.value)}
					on:keydown={(e) => handleOptionKeydown(e, option.value)}
				>
					{#if option.value === value}
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
					{option.label}
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.cs-wrap {
		flex: 1;
		min-width: 0;

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

		&--sm {
			height: $ctrl-height-sm;
			padding-left: 8px;
			font-size: 11px;
		}

		&--xs {
			height: $ctrl-height-xs;
			padding: 0 6px 0 8px;
			font-size: 10px;
		}

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
		position: fixed;
		box-sizing: border-box;
		overflow-y: auto;
		background: $surface-overlay;
		border: 1px solid $border-subtle;
		border-radius: $radius-lg;
		padding: 4px;
		margin: 0;
		list-style: none;
		z-index: $z-overlay;
		box-shadow: $shadow-overlay;
	}

	.cs-group {
		padding: 6px 8px 2px;
		font-family: $font-mono;
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: $text-subtle;
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
