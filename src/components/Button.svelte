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
	@use '../controls.scss' as *;

	.btn {
		@include button;
		padding: 0 14px;

		&--primary {
			@include button-primary;
			padding: 0 16px;
		}

		&--ghost {
			height: auto;
			padding: 0;
			border-color: transparent;
			background: none;
			color: $text-subtle;
			font-size: 11px;

			&:not(:disabled):hover {
				background: none;
				border-color: transparent;
				color: $text-secondary;
			}
		}

		&--compact {
			padding: 0 10px;
		}
	}
</style>
