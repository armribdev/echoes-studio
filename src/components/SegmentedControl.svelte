<script lang="ts">
	import { createEventDispatcher } from 'svelte';

	type OptionValue = string | number;
	type Option = { value: OptionValue; label: string; title?: string; disabled?: boolean };

	/** Selected value (bindable). */
	export let value: OptionValue;
	export let options: Option[];
	/** `sm` (24px) fits toolbars, `md` (30px) fits forms. */
	export let size: 'sm' | 'md' = 'md';
	export let ariaLabel: string | undefined = undefined;

	const dispatch = createEventDispatcher<{ change: OptionValue }>();

	function select(option: Option) {
		if (option.disabled || option.value === value) return;
		value = option.value;
		dispatch('change', option.value);
	}
</script>

<div class="segmented" class:segmented--sm={size === 'sm'} role="radiogroup" aria-label={ariaLabel}>
	{#each options as option (option.value)}
		<button
			class="segment"
			class:active={option.value === value}
			type="button"
			role="radio"
			title={option.title}
			aria-checked={option.value === value}
			disabled={option.disabled}
			on:click={() => select(option)}
		>
			<slot {option}>{option.label}</slot>
		</button>
	{/each}
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.segmented {
		@include segmented;
		max-width: 100%;
	}

	.segment {
		@include segment($ctrl-height - 4px, 12px);
	}

	.segmented--sm .segment {
		height: $ctrl-height-sm - 4px;
		padding: 0 ($ctrl-height-sm * 0.4);
		font-size: 11px;
	}
</style>
