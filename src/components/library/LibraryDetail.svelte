<script lang="ts">
	import { t } from 'svelte-i18n';
	import {
		formatBeats,
		formatDuration,
		isMidiLibraryKind,
		type LibraryAsset
	} from '../../frontend/library';

	export let asset: LibraryAsset | null;
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
	{:else}
		<div class="detail-placeholder">{$t('library.selectSound')}</div>
	{/if}
</div>

<style lang="scss">
	@use '../../variables.scss' as *;

	.detail {
		flex-shrink: 0;
		border-top: 1px solid $border-color;
		padding: 8px 10px;
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
		gap: 10px;
		margin-top: 3px;
		font-size: 9px;
		font-family: $font-mono;
		color: $text-faint;
	}

	.detail-tip {
		margin-top: 4px;
		font-size: 9px;
		color: $text-faint;
		opacity: 0.85;
	}

	.detail-placeholder {
		font-size: 10px;
		color: $text-faint;
	}
</style>
