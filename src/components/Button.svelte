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
			background 0.1s,
			color 0.1s,
			opacity 0.1s;
		white-space: nowrap;

		&:disabled {
			opacity: 0.4;
			cursor: not-allowed;
		}

		// ── Primary ─────────────────────────────────────────────────────────────
		&--primary {
			background: $accent-color;
			border: none;
			color: #fff;
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
			color: rgba(247, 247, 249, 0.75);
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
				color: rgba(247, 247, 249, 0.7);
			}
		}

		// ── Compact modifier ────────────────────────────────────────────────────
		&--compact {
			padding: 0 10px;
		}
	}
</style>
