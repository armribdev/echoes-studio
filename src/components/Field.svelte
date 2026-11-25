<script lang="ts">
	export let label = '';
	export let hint = '';
</script>

<div class="field">
	{#if label || $$slots.label}
		<span class="label"><slot name="label">{label}</slot></span>
	{/if}
	<slot />
	{#if hint || $$slots.hint}
		<span class="hint"><slot name="hint">{hint}</slot></span>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.field {
		display: flex;
		flex-direction: column;
		gap: 5px;
	}

	.label {
		font-size: 11px;
		font-weight: 500;
		line-height: 16px;
		letter-spacing: 0.3px;
		color: $text-muted;
	}

	.hint {
		font-size: 10px;
		line-height: 1.2;
		color: $text-faint;
	}
</style>
