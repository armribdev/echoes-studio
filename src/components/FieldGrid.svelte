<div class="field-grid"><slot /></div>

<style lang="scss">
	.field-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
		gap: 16px;
		margin-top: 4px;
	}
</style>
