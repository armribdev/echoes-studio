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
	$: fill = `${((value - min) / (max - min)) * 100}%`;
</script>

<div class="range-field">
	{#if label}
		<span class="range-label">{displayLabel}</span>
	{/if}
	<input class="range-input" type="range" {min} {max} {step} bind:value style:--fill={fill} />
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
	@use '../controls.scss' as *;

	.range-field {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.range-label {
		font-size: 11px;
		color: $text-muted;
		font-weight: 500;
	}

	.range-input {
		@include range-slider;
	}

	.range-hints {
		display: flex;
		justify-content: space-between;
		font-size: 9px;
		color: $text-faint;
	}
</style>
