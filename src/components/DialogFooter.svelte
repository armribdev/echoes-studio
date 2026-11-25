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
		padding: 12px 18px;
		background: $panel-background-color;
		border-top: 1px solid $border-color;
	}

	.actions {
		display: flex;
		gap: 8px;
		margin-left: auto;
	}
</style>
