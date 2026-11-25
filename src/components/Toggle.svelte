<script lang="ts">
	/** Valeur liée (cochée / non cochée). */
	export let checked: boolean;
	/** Libellé principal affiché à gauche. */
	export let label: string;
	/** Texte descriptif optionnel sous le libellé. */
	export let description: string = '';
	/** Désactiver le toggle. */
	export let disabled: boolean = false;
</script>

<div class="toggle-row" class:disabled>
	<div class="toggle-info">
		<slot name="info">
			<span class="toggle-label">{label}</span>
			{#if description}
				<span class="toggle-desc">{description}</span>
			{/if}
		</slot>
	</div>
	<button
		class="toggle-switch"
		class:on={checked}
		on:click={() => {
			if (!disabled) checked = !checked;
		}}
		role="switch"
		aria-checked={checked}
		aria-label={label}
		{disabled}
		type="button"
	>
		<span class="toggle-thumb"></span>
	</button>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.toggle-row {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 10px 12px;
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.07);
		border-radius: 6px;

		&.disabled {
			opacity: 0.45;
			pointer-events: none;
		}
	}

	.toggle-info {
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.toggle-label {
		font-size: 12px;
		font-weight: 500;
		color: $text-color;
	}

	.toggle-desc {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.35);
	}

	.toggle-switch {
		width: 36px;
		height: 20px;
		border-radius: $radius-xl;
		background: rgba(255, 255, 255, 0.1);
		border: 1px solid rgba(255, 255, 255, 0.12);
		cursor: pointer;
		position: relative;
		flex-shrink: 0;
		transition:
			background 0.2s,
			border-color 0.2s;
		padding: 0;

		&.on {
			background: $accent-color;
			border-color: $accent-color;

			.toggle-thumb {
				transform: translateX(16px);
			}
		}

		&:focus-visible {
			outline: 2px solid $accent-color;
			outline-offset: 2px;
		}
	}

	.toggle-thumb {
		position: absolute;
		top: 2px;
		left: 2px;
		width: 14px;
		height: 14px;
		border-radius: 50%;
		background: $text-color;
		transition: transform 0.2s;
		pointer-events: none;
	}
</style>
