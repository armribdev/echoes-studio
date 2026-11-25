<div class="field-grid"><slot /></div>

<style lang="scss">
	@use '../variables.scss' as *;
	.field-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
		gap: $space-6;
		margin-top: $space-2;
	}
</style>
