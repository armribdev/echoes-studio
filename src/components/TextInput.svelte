<script lang="ts">
	export let value = '';
	export let placeholder = '';
	export let maxlength: number | undefined = undefined;
</script>

<input class="text-input" type="text" bind:value {placeholder} {maxlength} />

<style lang="scss">
	@use '../variables.scss' as *;

	.text-input {
		flex: 1;
		height: $ctrl-height;
		padding: 5px 9px;
		box-sizing: border-box;
		background: $surface-2;
		border: 1px solid $border-subtle;
		border-radius: $radius-md;
		color: $text-color;
		font-size: 12px;
		font-family: inherit;
		outline: none;
		transition:
			border-color $duration-fast,
			background $duration-fast;

		&::placeholder {
			color: $text-faint;
		}

		&:focus {
			border-color: $accent-color;
			background: $accent-wash;
		}
	}
</style>
