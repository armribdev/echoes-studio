<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';

	export let label: string;
	export let format: string | null = null;
	export let enabled: boolean;
	export let isolated = false;
	export let missingLabel: string | null = null;
	export let missingHelp = '';
	export let crashed = false;
	export let open = false;
	export let reloading = false;
	export let compact = false;

	const FORMAT_COLOR: Record<string, string> = {
		VST3: '#1571ff',
		CLAP: '#22c55e',
		AU: '#f59e0b'
	};

	const dispatch = createEventDispatcher<{
		toggleBypass: void;
		toggleExpand: void;
		reload: void;
		remove: void;
	}>();
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	class="slot-head"
	class:compact
	class:unavailable={missingLabel !== null}
	class:crashed
	on:contextmenu
>
	<button
		class="bypass"
		class:on={enabled}
		type="button"
		on:click|stopPropagation={() => dispatch('toggleBypass')}
		title={$t('effects.bypass')}
	>
		<svg width="8" height="8" viewBox="0 0 8 8"
			><circle cx="4" cy="4" r="3" fill="currentColor" /></svg
		>
	</button>
	<button class="slot-toggle" type="button" on:click={() => dispatch('toggleExpand')}>
		<span class="slot-name">{label}</span>
		{#if format}
			<span class="fmt-badge" style:background={FORMAT_COLOR[format] ?? '#555'}>
				{format}
			</span>
		{/if}
		{#if isolated}
			<span class="sandbox-badge" title={$t('effects.isolated')}>ISO</span>
		{/if}
		{#if missingLabel !== null}
			<span class="missing-badge" title={missingHelp}>
				{missingLabel}
			</span>
		{/if}
		<svg class="chevron" class:open width="8" height="5" viewBox="0 0 8 5">
			<path
				d="M1 1 L4 4 L7 1"
				stroke="currentColor"
				stroke-width="1.5"
				fill="none"
				stroke-linecap="round"
			/>
		</svg>
	</button>
	{#if crashed}
		<button
			class="reload-btn"
			type="button"
			on:click|stopPropagation={() => dispatch('reload')}
			disabled={reloading}
			title={missingHelp}
		>
			{compact ? '↻' : reloading ? 'Rechargement…' : 'Recharger'}
		</button>
	{/if}
	<button
		class="rm-btn"
		type="button"
		on:click|stopPropagation={() => dispatch('remove')}
		title={$t('common.remove')}>×</button
	>
</div>

<style lang="scss">
	@use '../../variables.scss' as *;

	.slot-head {
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 4px 6px;
	}

	.slot-toggle {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 5px;
		min-width: 0;
		padding: 0;
		border: none;
		background: transparent;
		color: inherit;
		font: inherit;
		cursor: pointer;
		text-align: left;
		&:hover {
			background: $surface-1;
			border-radius: $radius-xs;
		}
	}

	.slot-head.compact {
		padding: 2px 5px;
	}

	.bypass {
		width: 13px;
		height: 13px;
		border-radius: 50%;
		border: 1px solid $border-subtle;
		background: transparent;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 0;
		flex-shrink: 0;
		color: $text-faint;
		&.on {
			color: $accent-color;
			border-color: $accent-color;
		}
	}

	.slot-name {
		flex: 1;
		font-size: 10px;
		font-weight: 500;
		color: $text-color;
		user-select: none;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.slot-head.compact .slot-name {
		font-size: 9px;
	}

	.fmt-badge {
		font-size: 7px;
		font-family: $font-mono;
		font-weight: 700;
		letter-spacing: 0.04em;
		padding: 1px 4px;
		border-radius: 2px;
		color: #fff;
		flex-shrink: 0;
	}

	.slot-head.unavailable .slot-name {
		color: $text-faint;
		text-decoration: line-through;
	}

	.slot-head.crashed .slot-name {
		color: #ff8a82;
		text-decoration: none;
	}

	.reload-btn {
		flex-shrink: 0;
		padding: 2px 8px;
		border: 1px solid rgba(255, 65, 54, 0.6);
		border-radius: $radius-xs;
		background: rgba(255, 65, 54, 0.18);
		color: #ffb3ad;
		font-size: 9px;
		font-family: $font-mono;
		font-weight: 700;
		cursor: pointer;
		&:hover:not(:disabled) {
			background: rgba(255, 65, 54, 0.32);
			color: #fff;
		}
		&:disabled {
			cursor: default;
			opacity: 0.6;
		}
	}

	.missing-badge {
		font-size: 7px;
		font-family: $font-mono;
		font-weight: 700;
		padding: 1px 4px;
		border-radius: 2px;
		flex-shrink: 0;
		white-space: nowrap;
		border: 1px solid rgba(255, 65, 54, 0.5);
		background: rgba(255, 65, 54, 0.12);
		color: #ff6b61;
	}

	.chevron {
		flex-shrink: 0;
		color: $text-faint;
		transition: transform 0.15s;
		&.open {
			transform: rotate(180deg);
		}
	}

	.rm-btn {
		width: 16px;
		height: 16px;
		border: none;
		background: transparent;
		color: $text-faint;
		font-size: 14px;
		line-height: 1;
		cursor: pointer;
		padding: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		flex-shrink: 0;
		&:hover {
			color: $color-error;
		}
	}

	.sandbox-badge {
		font-size: 7px;
		font-family: $font-mono;
		font-weight: 700;
		padding: 1px 4px;
		border-radius: 2px;
		flex-shrink: 0;
		border: 1px solid rgba(34, 197, 94, 0.5);
		background: rgba(34, 197, 94, 0.12);
		color: #4ade80;
	}
</style>
