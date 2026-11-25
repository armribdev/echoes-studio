<script lang="ts">
	export let variant: 'primary' | 'secondary' | 'ghost' = 'secondary';
	export let type: 'button' | 'submit' | 'reset' = 'button';
	export let disabled = false;
	export let compact = false;
	export let title = '';
</script>

<button {type} {disabled} {title} class="btn btn--{variant}" class:btn--compact={compact} on:click>
	<slot />
</button>

<style lang="scss">
	@use '../variables.scss' as *;

	.btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		height: $ctrl-height;
		border-radius: $radius-md;
		font-size: 12px;
		font-family: inherit;
		cursor: pointer;
		transition:
			background $duration-base,
			color $duration-base,
			opacity $duration-base;
		white-space: nowrap;

		&:disabled {
			opacity: 0.4;
			cursor: default;
		}

		// ── Primary ─────────────────────────────────────────────────────────────
		&--primary {
			background: $accent-color;
			border: none;
			color: $on-solid;
			font-weight: 600;
			padding: 0 16px;

			&:not(:disabled):hover {
				background: $accent-hover;
			}
			&:not(:disabled):active {
				opacity: 0.85;
			}
		}

		// ── Secondary ───────────────────────────────────────────────────────────
		&--secondary {
			background: $surface-2;
			border: 1px solid $border-subtle;
			color: $text-secondary;
			padding: 0 14px;

			&:not(:disabled):hover {
				background: $surface-hover;
				color: $text-color;
			}
		}

		// ── Ghost ───────────────────────────────────────────────────────────────
		&--ghost {
			background: none;
			border: none;
			color: $text-subtle;
			font-size: 11px;
			padding: 0;
			height: auto;

			&:not(:disabled):hover {
				color: $text-secondary;
			}
		}

		// ── Compact modifier ────────────────────────────────────────────────────
		&--compact {
			padding: 0 10px;
		}
	}
</style>
