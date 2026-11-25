<script lang="ts">
	export let title: string;
	export let tip = '';
	/** Smaller padding, for an empty list that stays visible. */
	export let compact = false;
</script>

<div class="empty-state" class:empty-state--compact={compact}>
	<p class="empty-title">{title}</p>
	{#if $$slots.default}
		<div class="empty-actions"><slot /></div>
	{/if}
	{#if tip}
		<p class="empty-tip">{tip}</p>
	{/if}
</div>

<style lang="scss">
	@use '../../variables.scss' as *;

	.empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: $space-5;
		padding: $space-7 $space-6;
		color: $text-faint;
		text-align: center;

		&--compact {
			padding: $space-7 $space-5;
			gap: $space-3;
		}
	}

	.empty-title {
		font-size: 12px;
		color: $text-subtle;
		margin: 0;
	}

	.empty-actions {
		display: flex;
		flex-direction: column;
		gap: $space-3;
		width: 100%;
		max-width: 160px;
	}

	.empty-tip {
		font-size: 10px;
		line-height: 1.45;
		margin: 0;
		color: $text-faint;
	}
</style>
