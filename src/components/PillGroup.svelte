<script lang="ts">
	type OptionValue = string | number;

	/** Valeur sélectionnée (bindable). */
	export let value: OptionValue;
	/** Liste des options disponibles. */
	export let options: Array<{ value: OptionValue; label: string; disabled?: boolean }>;
</script>

<div class="pill-group" role="group">
	{#each options as opt}
		<button
			class="pill"
			class:selected={opt.value === value}
			class:disabled={opt.disabled}
			type="button"
			role="radio"
			aria-checked={opt.value === value}
			aria-disabled={opt.disabled}
			disabled={opt.disabled}
			on:click={() => {
				if (!opt.disabled) value = opt.value;
			}}
		>
			{opt.label}
		</button>
	{/each}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.pill-group {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}

	.pill {
		font-size: 11px;
		font-family: $font-mono;
		padding: 4px 9px;
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: $radius-sm;
		background: rgba(255, 255, 255, 0.04);
		color: rgba(247, 247, 249, 0.6);
		cursor: pointer;
		white-space: nowrap;
		transition:
			background $duration-base,
			border-color $duration-base,
			color $duration-base,
			opacity $duration-base;

		&:hover:not(.disabled) {
			background: rgba(255, 255, 255, 0.08);
			color: $text-color;
		}

		&.selected {
			background: rgba(21, 113, 255, 0.2);
			border-color: $accent-color;
			color: $text-color;
		}

		&.disabled {
			opacity: 0.28;
			cursor: default;
			pointer-events: none;
		}

		&:focus-visible {
			outline: 2px solid $accent-color;
			outline-offset: 2px;
		}
	}
</style>
