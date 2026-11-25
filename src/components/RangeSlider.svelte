<script lang="ts">
	/** Valeur courante (bindable). */
	export let value: number;
	export let min: number;
	export let max: number;
	export let step: number = 0.01;
	/**
	 * Libellé affiché au-dessus du slider.
	 * Supporte le placeholder `{value}` qui sera remplacé dynamiquement.
	 * Ex. : "Zoom par défaut — {value}×"
	 */
	export let label: string = '';
	/**
	 * Marqueurs textuels alignés sous la barre (gauche → droite).
	 * Ex. : ['0.25×', '1×', '2×', '4×']
	 */
	export let hints: string[] = [];

	$: displayLabel = label.replace('{value}', value.toFixed(2));
</script>

<div class="range-field">
	{#if label}
		<span class="range-label">{displayLabel}</span>
	{/if}
	<input class="range-input" type="range" {min} {max} {step} bind:value />
	{#if hints.length}
		<div class="range-hints">
			{#each hints as h}
				<span>{h}</span>
			{/each}
		</div>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.range-field {
		display: flex;
		flex-direction: column;
		gap: 5px;
	}

	.range-label {
		font-size: 11px;
		color: rgba(247, 247, 249, 0.55);
		font-weight: 500;
		letter-spacing: 0.3px;
	}

	.range-input {
		width: 100%;
		-webkit-appearance: none;
		appearance: none;
		height: 4px;
		border-radius: 2px;
		background: rgba(255, 255, 255, 0.1);
		outline: none;
		cursor: pointer;

		&::-webkit-slider-thumb {
			-webkit-appearance: none;
			width: 14px;
			height: 14px;
			border-radius: 50%;
			background: $accent-color;
			cursor: pointer;
			transition:
				transform $duration-base,
				box-shadow $duration-base;

			&:hover {
				transform: scale(1.2);
				box-shadow: 0 0 0 4px rgba(21, 113, 255, 0.25);
			}
		}

		&::-moz-range-thumb {
			width: 14px;
			height: 14px;
			border-radius: 50%;
			background: $accent-color;
			border: none;
			cursor: pointer;
		}

		&:focus-visible::-webkit-slider-thumb {
			box-shadow: 0 0 0 4px rgba(21, 113, 255, 0.35);
		}
	}

	.range-hints {
		display: flex;
		justify-content: space-between;
		font-size: 9px;
		color: rgba(247, 247, 249, 0.25);
	}
</style>
