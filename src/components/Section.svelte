<script lang="ts">
	export let title: string;
</script>

<section class="section">
	<h2 class="title">{title}</h2>
	<slot />
</section>

<style lang="scss">
	@use '../variables.scss' as *;

	.section {
		display: flex;
		flex-direction: column;
		gap: $space-4;
	}

	.title {
		margin: 0 0 $space-2;
		font-size: 14px;
		font-weight: 600;
		letter-spacing: 0.1px;
		color: $text-color;
	}
</style>
