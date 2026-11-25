<script lang="ts">
	import { t } from 'svelte-i18n';
	import { statusMessage, pianoRollStatusInfo } from '../stores/status';
	import { viewMode } from '../stores/viewMode';
</script>

<div class="status-bar">
	<div class="status-left">
		{#if $statusMessage}
			<span class="message message--{$statusMessage.level}">{$statusMessage.text}</span>
		{/if}
	</div>

	<div class="view-tabs">
		<button
			type="button"
			class="view-tab"
			class:active={$viewMode.main === 'arrange'}
			on:click={() => viewMode.setMain('arrange')}
			title={$t('statusBar.arrangeTitle')}
		>
			{$t('statusBar.arrange')}
		</button>
		<button
			type="button"
			class="view-tab"
			class:active={$viewMode.main === 'graph'}
			on:click={() => viewMode.setMain('graph')}
			title={$t('statusBar.graphTitle')}
		>
			{$t('statusBar.graph')}
		</button>
	</div>

	<div class="status-right">
		{#if $pianoRollStatusInfo}
			{#if $pianoRollStatusInfo.hoverLabel}
				<span class="piano-hover">{$pianoRollStatusInfo.hoverLabel}</span>
			{/if}
			<span class="piano-meta">
				{$pianoRollStatusInfo.noteMeta} · {$pianoRollStatusInfo.clipMeta}
			</span>
		{/if}
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.status-bar {
		min-height: 24px;
		background-color: $panel-background-color;
		border-top: 1px solid $border-color;
		display: grid;
		grid-template-columns: 1fr auto 1fr;
		align-items: center;
		padding: 0 12px;
		flex-shrink: 0;
		position: relative;
	}

	.status-left {
		grid-column: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.view-tabs {
		grid-column: 2;
		display: flex;
		gap: 1px;
		padding: 1px;
		opacity: 0.55;
		transition: opacity 0.15s;

		&:hover {
			opacity: 0.85;
		}
	}

	.view-tab {
		padding: 3px 8px;
		border: none;
		border-radius: $radius-xs;
		background: transparent;
		color: rgba(247, 247, 249, 0.32);
		font-size: 10px;
		font-family: inherit;
		cursor: pointer;
		transition:
			background $duration-base,
			color $duration-base;

		&.active {
			background: rgba(21, 113, 255, 0.12);
			color: rgba(247, 247, 249, 0.62);
		}

		&:hover:not(.active) {
			color: rgba(247, 247, 249, 0.48);
		}
	}

	.status-right {
		grid-column: 3;
		justify-self: end;
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 0;
		overflow: hidden;
		white-space: nowrap;
	}

	.piano-hover {
		font-size: 11px;
		font-family: $font-mono;
		font-variant-numeric: tabular-nums;
		letter-spacing: 0.02em;
		color: rgba(170, 170, 200, 0.85);
	}

	.piano-meta {
		font-size: 11px;
		font-variant-numeric: tabular-nums;
		color: rgba(247, 247, 249, 0.38);
	}

	.message {
		font-size: 11px;
		font-family: $font-mono;
		letter-spacing: 0.3px;

		&--info {
			color: rgba(247, 247, 249, 0.5);
		}
		&--success {
			color: #4caf82;
		}
		&--error {
			color: #ff5f57;
		}
	}
</style>
