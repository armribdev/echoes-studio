<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';
	import {
		collectionSourceId,
		LIBRARY_SOURCE_ORDER,
		packLabel,
		packSourceId,
		type LibraryCollection,
		type LibrarySourceId
	} from '../../frontend/library';

	export let source: LibrarySourceId;
	export let collections: readonly LibraryCollection[];
	export let indexedFolders: readonly string[];

	const dispatch = createEventDispatcher<{ select: LibrarySourceId; addFolder: void }>();
</script>

<nav class="sources" aria-label={$t('library.sourcesAria')}>
	{#each LIBRARY_SOURCE_ORDER as sourceId}
		<button
			type="button"
			class="source-btn"
			class:active={source === sourceId}
			on:click={() => dispatch('select', sourceId)}
		>
			{$t(`library.sources.${sourceId}`)}
		</button>
	{/each}
	{#each collections as collection (collection.id)}
		<button
			type="button"
			class="source-btn source-btn--named"
			class:active={source === collectionSourceId(collection.id)}
			title={collection.name}
			on:click={() => dispatch('select', collectionSourceId(collection.id))}
		>
			{collection.name}
		</button>
	{/each}
	{#each indexedFolders as folder (folder)}
		<button
			type="button"
			class="source-btn source-btn--named"
			class:active={source === packSourceId(folder)}
			title={folder}
			on:click={() => dispatch('select', packSourceId(folder))}
		>
			{packLabel(folder)}
		</button>
	{/each}
	<button
		type="button"
		class="source-btn source-btn--action"
		on:click={() => dispatch('addFolder')}
	>
		{$t('library.addFolder')}
	</button>
</nav>

<style lang="scss">
	@use '../../variables.scss' as *;
	@use '../../controls.scss' as *;

	.sources {
		flex-shrink: 0;
		display: flex;
		flex-direction: row;
		align-items: center;
		gap: $space-1;
		padding: 0 $panel-inset $space-3;
		overflow-x: auto;
		border-bottom: 1px solid $border-color;
	}

	.source-btn {
		@include segment($ctrl-height-sm, 11px);
		flex-shrink: 0;

		&--named {
			max-width: 140px;
			overflow: hidden;
			text-overflow: ellipsis;
			white-space: nowrap;
		}

		&--action {
			margin-left: auto;
			color: $accent-text;
		}
	}
</style>
