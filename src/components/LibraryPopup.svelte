<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { dismissable } from '../frontend/dismiss';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import { notify } from '../stores/status';
	import {
		libraryStore,
		libraryVisibleAssets,
		libraryHasIndexedContent,
		libraryFocusSearchTick,
		libraryPopup,
		insertLibraryAsset,
		LIBRARY_SOURCE_LABELS,
		LIBRARY_SOURCE_ORDER,
		type LibraryAsset,
		type LibraryOpenContext,
		type LibrarySourceId,
	} from '../frontend/library';

	let rootEl: HTMLDivElement | null = null;
	let searchInputEl: HTMLInputElement | null = null;
	let resultsEl: HTMLDivElement | null = null;
	let adjustedX = 0;
	let adjustedY = 0;
	let lastFocusTick = 0;

	$: visible = $libraryVisibleAssets;
	$: popup = $libraryPopup;

	const MAX_ROWS = 10;
	$: shown = visible.slice(0, MAX_ROWS);

	$: if ($libraryFocusSearchTick !== lastFocusTick) {
		lastFocusTick = $libraryFocusSearchTick;
		requestAnimationFrame(() => {
			searchInputEl?.focus();
			searchInputEl?.select();
			keyboardFocus.setLibrarySearch();
		});
	}

	function defaultSourceForContext(context: LibraryOpenContext): LibrarySourceId {
		switch (context) {
			case 'track_empty':
			case 'track_audio_lane':
			case 'playhead_insert':
			default:
				return 'audio';
		}
	}

	function contextLabel(context: LibraryOpenContext): string {
		switch (context) {
			case 'track_empty':
				return 'piste vide';
			case 'track_audio_lane':
				return 'audio';
			case 'playhead_insert':
				return 'playhead';
			default:
				return context;
		}
	}

	onMount(() => {
		libraryStore.setSource(defaultSourceForContext(popup.context));
		libraryStore.clearQuery();
		void tick().then(() => {
			if (rootEl) {
				const rect = rootEl.getBoundingClientRect();
				const vw = window.innerWidth;
				const vh = window.innerHeight;
				adjustedX = Math.min(popup.anchorX, vw - rect.width - 8);
				adjustedY = Math.min(popup.anchorY, vh - rect.height - 8);
				adjustedX = Math.max(8, adjustedX);
				adjustedY = Math.max(8, adjustedY);
			}
			searchInputEl?.focus();
			searchInputEl?.select();
			keyboardFocus.setLibrarySearch();
		});
	});

	function close() {
		libraryStore.stopPreview();
		libraryPopup.close();
	}

	function moveSelection(delta: number) {
		libraryStore.moveSelection(
			delta,
			shown.map((a) => a.id),
		);
	}

	async function insert(asset: LibraryAsset) {
		const ok = await insertLibraryAsset({
			asset,
			trackId: popup.trackId,
		});
		if (ok) close();
	}

	function onRowClick(asset: LibraryAsset) {
		libraryStore.select(asset.id);
		keyboardFocus.setLibraryResults();
	}

	function onFavoriteClick(e: MouseEvent, asset: LibraryAsset) {
		e.stopPropagation();
		libraryStore.toggleFavorite(asset.id);
	}

	async function onPreviewClick(e: MouseEvent, asset: LibraryAsset) {
		e.stopPropagation();
		libraryStore.select(asset.id);
		keyboardFocus.setLibraryResults();
		try {
			await libraryStore.playPreview(asset.id);
		} catch (err) {
			notify(`Aperçu : ${(err as Error)?.message ?? err}`, 'error');
		}
	}

	function onResultsKeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			moveSelection(1);
			return;
		}
		if (e.key === 'ArrowUp') {
			e.preventDefault();
			moveSelection(-1);
			return;
		}
		if (e.key === 'Enter' && $libraryStore.selectedId) {
			e.preventDefault();
			const asset = shown.find((a) => a.id === $libraryStore.selectedId);
			if (asset) void insert(asset);
			return;
		}
		if (e.key === ' ' && $libraryStore.selectedId) {
			e.preventDefault();
			void libraryStore.togglePreview($libraryStore.selectedId).catch((err) => {
				notify(`Aperçu : ${(err as Error)?.message ?? err}`, 'error');
			});
			return;
		}
		if (e.key === 'f' || e.key === 'F') {
			if ($libraryStore.selectedId) {
				e.preventDefault();
				libraryStore.toggleFavorite($libraryStore.selectedId);
			}
			return;
		}
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
			if ($libraryStore.query) {
				libraryStore.clearQuery();
			} else {
				close();
			}
		}
	}

	function onSearchKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
			if ($libraryStore.query) {
				libraryStore.clearQuery();
			} else {
				close();
			}
			return;
		}
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			resultsEl?.focus();
			keyboardFocus.setLibraryResults();
			moveSelection(1);
			return;
		}
		if (e.key === 'Enter' && $libraryStore.selectedId) {
			e.preventDefault();
			const asset = shown.find((a) => a.id === $libraryStore.selectedId);
			if (asset) void insert(asset);
		}
	}

	function setSource(id: LibrarySourceId) {
		libraryStore.setSource(id);
		keyboardFocus.setLibraryResults();
	}
</script>

<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
<div
	class="library-popup"
	bind:this={rootEl}
	use:dismissable={close}
	role="dialog"
	aria-label="Insérer depuis la bibliothèque"
	style="left: {adjustedX || popup.anchorX}px; top: {adjustedY || popup.anchorY}px"
	on:keydown={onResultsKeydown}
>
	<div class="popup-chrome">
		<span class="popup-title">Insérer</span>
		<span class="popup-ctx">{contextLabel(popup.context)}</span>
	</div>

	<div class="search-wrap">
		<svg class="search-icon" width="11" height="11" viewBox="0 0 11 11" fill="none">
			<circle cx="4.5" cy="4.5" r="3.5" stroke="currentColor" stroke-width="1.2" />
			<path d="M7.5 7.5L10 10" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
		</svg>
		<input
			class="search-input"
			type="text"
			placeholder="Rechercher…"
			value={$libraryStore.query}
			bind:this={searchInputEl}
			on:input={(e) => libraryStore.setQuery(e.currentTarget.value)}
			on:focus={() => keyboardFocus.setLibrarySearch()}
			on:keydown={onSearchKeydown}
		/>
		{#if $libraryStore.query}
			<button class="search-clear" type="button" on:click={() => libraryStore.clearQuery()}>×</button>
		{/if}
	</div>

	<nav class="quick-sources" aria-label="Sources rapides">
		{#each LIBRARY_SOURCE_ORDER as sourceId}
			<button
				type="button"
				class="source-chip"
				class:active={$libraryStore.source === sourceId}
				on:click={() => setSource(sourceId)}
			>
				{LIBRARY_SOURCE_LABELS[sourceId]}
			</button>
		{/each}
	</nav>

	<!-- svelte-ignore a11y-no-noninteractive-tabindex -->
	<div
		class="results"
		bind:this={resultsEl}
		tabindex="0"
		role="listbox"
		aria-label="Résultats"
		on:focus={() => keyboardFocus.setLibraryResults()}
		on:mousedown={() => keyboardFocus.setLibraryResults()}
	>
		{#if !$libraryHasIndexedContent && shown.length === 0}
			<p class="empty">Aucun son indexé — ouvrez la bibliothèque (B) pour ajouter un dossier.</p>
		{:else if shown.length === 0}
			<p class="empty">Aucun résultat</p>
		{:else}
			{#each shown as asset (asset.id)}
				<!-- svelte-ignore a11y-no-noninteractive-element-interactions a11y-click-events-have-key-events -->
				<div
					class="row"
					class:selected={$libraryStore.selectedId === asset.id}
					class:previewing={$libraryStore.previewingId === asset.id}
					role="option"
					aria-selected={$libraryStore.selectedId === asset.id}
					on:click={() => onRowClick(asset)}
					on:dblclick={() => insert(asset)}
				>
					<button
						class="fav-btn"
						type="button"
						class:on={$libraryStore.favoriteIds.includes(asset.id)}
						title="Favori (F)"
						on:click={(e) => onFavoriteClick(e, asset)}
					>
						{$libraryStore.favoriteIds.includes(asset.id) ? '★' : '☆'}
					</button>
					<span class="row-name">{asset.name}</span>
					<button
						class="preview-btn"
						type="button"
						title="Aperçu"
						on:click={(e) => onPreviewClick(e, asset)}
					>
						▶
					</button>
				</div>
			{/each}
			{#if visible.length > MAX_ROWS}
				<p class="more">+ {visible.length - MAX_ROWS} autres — affinez la recherche</p>
			{/if}
		{/if}
	</div>

	<div class="footer">
		<span>Entrée = insérer</span>
		<span>Esc = fermer</span>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.library-popup {
		position: fixed;
		z-index: 220;
		width: 320px;
		max-height: min(420px, calc(100vh - 24px));
		display: flex;
		flex-direction: column;
		background: #26262e;
		border: 1px solid $border-color;
		border-radius: $radius-lg;
		box-shadow: 0 8px 32px rgba(0, 0, 0, 0.55), 0 2px 8px rgba(0, 0, 0, 0.3);
		overflow: hidden;
		user-select: none;
	}

	.popup-chrome {
		display: flex;
		align-items: baseline;
		gap: 8px;
		padding: 8px 10px 4px;
	}

	.popup-title {
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.4px;
		text-transform: uppercase;
		color: $text-muted;
		font-family: $font-mono;
	}

	.popup-ctx {
		font-size: 10px;
		color: $text-faint;
	}

	.search-wrap {
		display: flex;
		align-items: center;
		gap: 6px;
		margin: 4px 8px 6px;
		padding: 0 8px;
		height: 28px;
		background: $surface-2;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
	}

	.search-icon {
		flex-shrink: 0;
		color: $text-subtle;
	}

	.search-input {
		flex: 1;
		min-width: 0;
		height: 100%;
		border: none;
		background: transparent;
		color: $text-color;
		font-size: 12px;
		outline: none;

		&::placeholder {
			color: $text-faint;
		}
	}

	.search-clear {
		border: none;
		background: transparent;
		color: $text-subtle;
		cursor: pointer;
		font-size: 14px;
		line-height: 1;
		padding: 0 2px;

		&:hover {
			color: $text-color;
		}
	}

	.quick-sources {
		display: flex;
		gap: 4px;
		padding: 0 8px 6px;
	}

	.source-chip {
		height: 22px;
		padding: 0 8px;
		border: 1px solid $border-subtle;
		border-radius: 999px;
		background: transparent;
		color: $text-muted;
		font-size: 10px;
		cursor: pointer;

		&:hover {
			color: $text-color;
			border-color: rgba($text-color, 0.35);
		}

		&.active {
			background: rgba($accent-color, 0.18);
			border-color: $accent-color;
			color: $text-color;
		}
	}

	.results {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		outline: none;
		padding: 0 4px 4px;
	}

	.row {
		display: flex;
		align-items: center;
		gap: 4px;
		height: 26px;
		padding: 0 6px;
		border-radius: $radius-sm;
		cursor: default;

		&:hover {
			background: $surface-hover;
		}

		&.selected {
			background: rgba($accent-color, 0.22);
		}

		&.previewing .row-name {
			color: $accent-hover;
		}
	}

	.fav-btn,
	.preview-btn {
		flex-shrink: 0;
		width: 20px;
		height: 20px;
		padding: 0;
		border: none;
		background: transparent;
		color: $text-faint;
		cursor: pointer;
		font-size: 11px;
		line-height: 1;

		&:hover {
			color: $text-color;
		}

		&.on {
			color: $color-warning;
		}
	}

	.row-name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 12px;
		color: $text-color;
	}

	.empty,
	.more {
		margin: 8px;
		font-size: 11px;
		color: $text-subtle;
		line-height: 1.4;
	}

	.footer {
		display: flex;
		justify-content: space-between;
		padding: 6px 10px;
		border-top: 1px solid $border-faint;
		font-size: 10px;
		color: $text-faint;
		font-family: $font-mono;
	}
</style>
