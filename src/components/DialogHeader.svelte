<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';
	import Icon from './Icon.svelte';

	export let title: string;

	const dispatch = createEventDispatcher<{ close: void }>();
</script>

<div class="header">
	{#if $$slots.icon}
		<span class="icon"><slot name="icon" /></span>
	{/if}
	<span class="title">{title}</span>
	<button
		class="close"
		type="button"
		title={$t('common.closeEsc')}
		on:click={() => dispatch('close')}
	>
		<Icon name="close" size={15} strokeWidth={1.5} />
	</button>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.header {
		display: flex;
		align-items: center;
		gap: 9px;
		flex-shrink: 0;
		padding: 14px 18px;
		background: $panel-background-color;
		border-bottom: 1px solid $border-color;
	}

	.icon {
		display: flex;
		flex-shrink: 0;
		color: $text-subtle;
	}

	.title {
		flex: 1;
		min-width: 0;
		font-size: 13px;
		font-weight: 600;
		letter-spacing: 0.2px;
		color: $text-color;
	}

	.close {
		display: flex;
		align-items: center;
		justify-content: center;
		width: $ctrl-height-sm;
		height: $ctrl-height-sm;
		border: none;
		border-radius: $radius-sm;
		background: none;
		color: $text-subtle;
		cursor: pointer;
		transition:
			background $duration-fast,
			color $duration-fast;

		&:hover {
			background: $surface-hover;
			color: $text-color;
		}
	}
</style>
