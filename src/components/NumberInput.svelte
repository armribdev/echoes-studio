<script lang="ts">
	import { createEventDispatcher } from 'svelte';

	/** Valeur numérique (bindable). */
	export let value: number;
	export let min: number | undefined = undefined;
	export let max: number | undefined = undefined;
	export let step: number = 1;
	/** Largeur CSS du champ (ex. "48px", "60px"). */
	export let width: string = '60px';
	/** Unité affichée à droite (ex. "BPM", "sec"). */
	export let unit: string = '';
	/** Taille visuelle : "sm" = barre d'outils, "md" = formulaire. */
	export let size: 'sm' | 'md' = 'md';
	export let disabled: boolean = false;

	const dispatch = createEventDispatcher<{ change: number }>();

	function handleChange(e: Event) {
		const input = e.target as HTMLInputElement;
		const v = parseFloat(input.value);
		if (!isNaN(v)) {
			const clamped = Math.min(
				max !== undefined ? max : Infinity,
				Math.max(min !== undefined ? min : -Infinity, v)
			);
			value = clamped;
			input.value = String(clamped);
			dispatch('change', clamped);
		} else {
			input.value = String(value);
		}
	}
</script>

<div class="num-wrap" class:has-unit={!!unit}>
	<input
		class="num-input"
		class:num-input--sm={size === 'sm'}
		type="number"
		{min}
		{max}
		{step}
		{disabled}
		{value}
		style="width: {width}"
		on:change={handleChange}
	/>
	{#if unit}
		<span class="num-unit">{unit}</span>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.num-wrap {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.num-input {
		@include field($ctrl-height, 13px);
		padding: 0 6px;
		font-family: $font-mono;
		text-align: center;
		-moz-appearance: textfield;
		appearance: textfield;

		&::-webkit-inner-spin-button,
		&::-webkit-outer-spin-button {
			-webkit-appearance: none;
		}

		&--sm {
			height: $ctrl-height-sm;
			font-size: 12px;
		}
	}

	.num-unit {
		font-size: 10px;
		color: $text-subtle;
		flex-shrink: 0;
	}
</style>
