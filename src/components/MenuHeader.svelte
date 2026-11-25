<div class="menu-header"><slot /></div>

<style lang="scss">
	@use '../variables.scss' as *;

	.menu-header {
		padding: 4px 10px 6px;
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.6px;
		text-transform: uppercase;
		color: $text-faint;
		font-family: $font-mono;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
