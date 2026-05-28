<script lang="ts">
	import { statusMessage } from '../stores/status';
</script>

<div class="status-bar">
	{#if $statusMessage}
		<span class="message message--{$statusMessage.level}">{$statusMessage.text}</span>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.status-bar {
		min-height: 24px;
		background-color: $panel-background-color;
		border-top: 1px solid $border-color;
		display: flex;
		align-items: center;
		padding: 0 12px;
		flex-shrink: 0;
	}

	.message {
		font-size: 11px;
		font-family: 'SF Mono', 'Fira Code', ui-monospace, monospace;
		letter-spacing: 0.3px;

		&--info    { color: rgba(247, 247, 249, 0.5); }
		&--success { color: #4caf82; }
		&--error   { color: #ff5f57; }
	}
</style>
