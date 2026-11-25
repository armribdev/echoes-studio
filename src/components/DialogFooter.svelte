<div class="footer">
	<slot name="start" />
	<div class="actions"><slot /></div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.footer {
		display: flex;
		align-items: center;
		flex-shrink: 0;
		padding: $space-5 $space-6;
		background: $panel-background-color;
		border-top: 1px solid $border-color;
	}

	.actions {
		display: flex;
		gap: $space-4;
		margin-left: auto;
	}
</style>
