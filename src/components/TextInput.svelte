<script lang="ts">
	export let value = '';
	export let placeholder = '';
	export let maxlength: number | undefined = undefined;
</script>

<input class="text-input" type="text" bind:value {placeholder} {maxlength} />

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.text-input {
		@include field;
		flex: 1;
	}
</style>
