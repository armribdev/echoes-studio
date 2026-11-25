<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';
	import TextInput from '../TextInput.svelte';
	import {
		formatBeats,
		formatDuration,
		isMidiLibraryKind,
		MAX_TAG_LENGTH,
		type LibraryAsset
	} from '../../frontend/library';

	export let asset: LibraryAsset | null;
	export let userTags: readonly string[] = [];

	const dispatch = createEventDispatcher<{ addTag: string; removeTag: string }>();

	let draft = '';

	function submitTag() {
		dispatch('addTag', draft);
		draft = '';
	}
</script>

<div class="detail">
	{#if asset}
		<div class="detail-name" title={asset.path}>{asset.name}</div>
		<div class="detail-meta">
			<span>{asset.extension.toUpperCase()}</span>
			{#if isMidiLibraryKind(asset.kind)}
				<span>{formatBeats(asset.lengthBeats)}</span>
				{#if asset.keyRoot}
					<span>{asset.keyRoot}</span>
				{/if}
			{:else}
				<span>{formatDuration(asset.durationSec)}</span>
			{/if}
		</div>
		{#if asset.kind === 'loop' || asset.kind === 'one_shot'}
			<div class="detail-tip">Placé à la longueur d'origine (pas de warp)</div>
		{/if}
		<div class="detail-tags">
			{#each userTags as tag (tag)}
				<button
					type="button"
					class="tag"
					title={$t('library.removeTag', { values: { tag } })}
					on:click={() => dispatch('removeTag', tag)}>{tag} ×</button
				>
			{/each}
		</div>
		<form class="tag-form" on:submit|preventDefault={submitTag}>
			<TextInput
				bind:value={draft}
				placeholder={$t('library.tagPlaceholder')}
				maxlength={MAX_TAG_LENGTH}
			/>
		</form>
	{:else}
		<div class="detail-placeholder">{$t('library.selectSound')}</div>
	{/if}
</div>

<style lang="scss">
	@use '../../variables.scss' as *;
	@use '../../controls.scss' as *;

	.detail {
		flex-shrink: 0;
		border-top: 1px solid $border-color;
		padding: $space-4 $space-5;
		min-height: 44px;
	}

	.detail-name {
		font-size: 11px;
		color: $text-color;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.detail-meta {
		display: flex;
		gap: $space-5;
		margin-top: $space-2;
		font-size: 9px;
		font-family: $font-mono;
		color: $text-faint;
	}

	.detail-tip {
		margin-top: $space-2;
		font-size: 9px;
		color: $text-faint;
		opacity: 0.85;
	}

	.detail-tags {
		display: flex;
		flex-wrap: wrap;
		gap: $space-2;
		margin-top: $space-3;

		&:empty {
			display: none;
		}
	}

	.tag {
		@include chip;
	}

	.tag-form {
		display: flex;
		margin-top: $space-3;
	}

	.detail-placeholder {
		font-size: 10px;
		color: $text-faint;
	}
</style>
