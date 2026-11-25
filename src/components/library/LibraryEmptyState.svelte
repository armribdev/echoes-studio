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
		gap: 12px;
		padding: 28px 14px;
		color: $text-faint;
		text-align: center;

		&--compact {
			padding: 24px 12px;
			gap: 6px;
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
		gap: 6px;
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
