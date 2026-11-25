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
	@use '../controls.scss' as *;

	.header {
		display: flex;
		align-items: center;
		gap: $space-4;
		flex-shrink: 0;
		padding: $space-6;
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
		@include icon-button;
	}
</style>
