<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';
	import type { LibraryAsset } from '../../frontend/library';

	export let asset: LibraryAsset;
	export let selected = false;
	export let previewing = false;
	export let importing = false;
	export let dragging = false;
	export let favorite = false;

	const dispatch = createEventDispatcher<{
		favorite: void;
		preview: void;
		insert: void;
		cancelImport: void;
	}>();
</script>

<div
	class="file-item"
	class:selected
	class:previewing
	class:importing
	class:dragging
	role="option"
	tabindex="-1"
	aria-selected={selected}
	aria-busy={importing}
	title={asset.path}
	on:click
	on:keydown
	on:dblclick
	on:pointerenter
	on:pointerleave
	on:pointerdown
	on:pointermove
	on:pointerup
	on:pointercancel
>
	<button
		class="fav-btn"
		type="button"
		class:on={favorite}
		title={$t('library.favorite')}
		on:click|stopPropagation={() => dispatch('favorite')}
		on:pointerdown|stopPropagation
	>
		{favorite ? '★' : '☆'}
	</button>
	<span class="file-ext">{asset.extension.toUpperCase()}</span>
	<span class="file-name">{asset.name}</span>
	{#if importing}
		<button
			class="cancel-import-btn"
			type="button"
			title={$t('library.cancelImport')}
			aria-label={$t('library.cancelImport')}
			on:pointerdown|stopPropagation
			on:click|stopPropagation={() => dispatch('cancelImport')}
		>
			<span class="cancel-import-ring" aria-hidden="true"></span>
			<span class="cancel-import-square" aria-hidden="true"></span>
		</button>
	{:else}
		{#if !previewing}
			<button
				class="preview-btn"
				type="button"
				title={$t('library.preview')}
				on:pointerdown|stopPropagation
				on:click|stopPropagation={() => dispatch('preview')}
			>
				▶
			</button>
		{/if}
		<button
			class="insert-btn"
			type="button"
			title={$t('library.insertAtPlayhead')}
			on:pointerdown|stopPropagation
			on:click={() => dispatch('insert')}
		>
			<svg width="8" height="8" viewBox="0 0 8 8" fill="none">
				<path
					d="M4 1V6M1.5 5.5L4 7.5L6.5 5.5"
					stroke="currentColor"
					stroke-width="1.3"
					stroke-linecap="round"
					stroke-linejoin="round"
				/>
			</svg>
		</button>
	{/if}
</div>

<style lang="scss">
	@use '../../variables.scss' as *;

	.file-item {
		display: flex;
		align-items: center;
		gap: 4px;
		box-sizing: border-box;
		height: 24px;
		padding: 0 6px 0 4px;
		cursor: grab;
		user-select: none;
		touch-action: none;
		transition: background $duration-base;

		&:hover,
		&.selected {
			background: $surface-hover;

			.insert-btn,
			.preview-btn {
				opacity: 1;
			}
		}

		&.selected {
			background: rgba(21, 113, 255, 0.12);
		}

		&.previewing .file-name {
			color: $text-color;
		}

		&.importing {
			background: rgba(21, 113, 255, 0.1);
			cursor: progress;

			.file-name {
				color: $text-color;
			}
			.cancel-import-btn {
				opacity: 1;
			}
		}

		&.dragging,
		&:active {
			cursor: grabbing;
			opacity: 0.85;
		}
	}

	.fav-btn {
		width: 16px;
		height: 16px;
		padding: 0;
		border: none;
		background: transparent;
		color: $text-faint;
		cursor: pointer;
		font-size: 10px;
		line-height: 1;
		flex-shrink: 0;

		&.on,
		&:hover {
			color: $color-warning;
		}
	}

	.file-ext {
		font-size: 7px;
		font-family: $font-mono;
		font-weight: 700;
		color: $accent-color;
		background: rgba(21, 113, 255, 0.12);
		border-radius: 2px;
		padding: 1px 3px;
		flex-shrink: 0;
		line-height: 1.4;
		letter-spacing: 0.05em;
	}

	.file-name {
		flex: 1;
		font-size: 11px;
		color: $text-subtle;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
	}

	.preview-btn,
	.cancel-import-btn,
	.insert-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
		background: transparent;
		border: none;
		padding: 0;
		color: $text-faint;
		cursor: pointer;
		opacity: 0;
		flex-shrink: 0;
		transition:
			color $duration-base,
			opacity $duration-base;
		font-size: 9px;
		position: relative;

		&:hover {
			color: $accent-color;
		}
	}

	.cancel-import-btn {
		opacity: 1;
		color: $accent-color;

		&:hover .cancel-import-square {
			background: $text-color;
		}
	}

	.cancel-import-ring {
		position: absolute;
		inset: 2px;
		border: 1.5px solid rgba(21, 113, 255, 0.28);
		border-top-color: $accent-color;
		border-radius: 50%;
		animation: library-spin 0.7s linear infinite;
	}

	.cancel-import-square {
		position: relative;
		width: 5px;
		height: 5px;
		border-radius: 0.5px;
		background: $accent-color;
		z-index: 1;
	}

	@keyframes library-spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
