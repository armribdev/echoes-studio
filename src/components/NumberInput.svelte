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
	/** Taille visuelle : "sm" = MenuBar (h 26px), "md" = panneau (h 28px). */
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

	.num-wrap {
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.num-input {
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: 4px;
		color: $text-color;
		font-family: 'SF Mono', 'Fira Code', ui-monospace, monospace;
		font-size: 13px;
		padding: 3px 6px;
		height: 28px;
		box-sizing: border-box;
		text-align: center;
		outline: none;
		-moz-appearance: textfield;
		line-height: 1;

		&::-webkit-inner-spin-button,
		&::-webkit-outer-spin-button {
			-webkit-appearance: none;
		}

		&:focus {
			border-color: $accent-color;
			background: rgba(21, 113, 255, 0.08);
		}

		&:disabled {
			opacity: 0.4;
			cursor: default;
		}

		/* MenuBar variant — slightly smaller */
		&--sm {
			font-size: 14px;
			height: 26px;
			border-radius: 3px;
			padding: 2px 4px;
		}
	}

	.num-unit {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.35);
		font-family: sans-serif;
		flex-shrink: 0;
	}
</style>
