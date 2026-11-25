<script lang="ts">
	/** Heading of the group; the divider above is drawn either way. */
	export let title = '';
</script>

<div class="subsection">
	<div class="divider"></div>
	{#if title}
		<h3 class="title">{title}</h3>
	{/if}
	<slot />
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.subsection {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.divider {
		height: 1px;
		margin: 8px 0;
		background: $border-color;
	}

	.title {
		margin: 0;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.8px;
		text-transform: uppercase;
		color: $text-subtle;
	}
</style>
