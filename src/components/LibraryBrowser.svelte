<script lang="ts">
	import SearchInput from './SearchInput.svelte';
	import { onMount } from 'svelte';
	import { notify } from '../stores/status';
	import { open as openDialog } from '@tauri-apps/plugin-dialog';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import { cancelAudioImport } from '../frontend/audio/importClip';
	import { audioImportProgress } from '../frontend/audio/importProgress';
	import {
		libraryStore,
		libraryVisibleAssets,
		libraryHasIndexedContent,
		libraryFocusSearchTick,
		insertLibraryAsset,
		LIBRARY_SOURCE_ORDER,
		AUDIO_EXTENSIONS,
		isMidiLibraryKind,
		libraryAssetLengthSec,
		beginLibraryDrag,
		endLibraryDrag,
		moveAudioFileDragPointer,
		requestAudioFileDragDrop,
		type LibraryAsset,
		type LibrarySourceId
	} from '../frontend/library';
	import { tempo } from '../stores/tempo';
	import { ARRANGE_POINTER_DRAG_THRESHOLD } from '../frontend/arrangeInteraction';
	import { t } from 'svelte-i18n';
	import LibraryAssetRow from './library/LibraryAssetRow.svelte';
	import LibraryDetail from './library/LibraryDetail.svelte';
	import LibraryEmptyState from './library/LibraryEmptyState.svelte';

	/** Docked panel vs Bitwig-style pop-up (same browser, different shell). */
	export let variant: 'dock' | 'popup' = 'dock';
	/** Focus search when mounted / when the shared focus tick fires. */
	export let autofocus = false;
	/** Target track for Enter/insert; `null` creates a track if needed. */
	export let insertTrackId: string | null | undefined = undefined;
	export let onInserted: (() => void) | undefined = undefined;
	/** Pop-up: Esc with empty query (or close request). */
	export let onRequestClose: (() => void) | undefined = undefined;

	let searchInputEl: HTMLInputElement | undefined;
	let resultsEl: HTMLDivElement | null = null;

	$: visible = $libraryVisibleAssets;
	$: selected = $libraryStore.selectedId
		? (visible.find((a) => a.id === $libraryStore.selectedId) ??
			$libraryStore.assetsById[$libraryStore.selectedId] ??
			null)
		: null;

	const ROW_HEIGHT = 24;
	const LIST_PADDING = 4;
	const ROW_OVERSCAN = 10;
	let scrollTop = 0;
	let viewportHeight = 0;

	$: firstRow = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - ROW_OVERSCAN);
	$: lastRow = Math.min(
		visible.length,
		Math.ceil((scrollTop + viewportHeight) / ROW_HEIGHT) + ROW_OVERSCAN
	);
	$: renderedRows = visible.slice(firstRow, lastRow);

	function moveSelection(delta: number) {
		libraryStore.moveSelection(
			delta,
			visible.map((a) => a.id)
		);
		scrollSelectionIntoView();
	}

	function scrollSelectionIntoView() {
		const selectedId = libraryStore.getSelectedAsset()?.id;
		if (!selectedId || !resultsEl) return;
		const index = visible.findIndex((a) => a.id === selectedId);
		if (index < 0) return;
		const rowTop = LIST_PADDING + index * ROW_HEIGHT;
		const rowBottom = rowTop + ROW_HEIGHT;
		if (rowTop < resultsEl.scrollTop) {
			resultsEl.scrollTop = rowTop;
		} else if (rowBottom > resultsEl.scrollTop + resultsEl.clientHeight) {
			resultsEl.scrollTop = rowBottom - resultsEl.clientHeight;
		}
	}

	let lastFocusTick = 0;
	$: if (autofocus && $libraryFocusSearchTick !== lastFocusTick) {
		lastFocusTick = $libraryFocusSearchTick;
		requestAnimationFrame(() => focusSearch());
	}

	function focusSearch() {
		searchInputEl?.focus();
		searchInputEl?.select();
		keyboardFocus.setLibrarySearch();
	}

	onMount(() => {
		libraryStore.hydrateFromPersistence();
		if (autofocus) {
			requestAnimationFrame(() => focusSearch());
		}
	});

	async function browseFiles() {
		try {
			const result = await openDialog({
				multiple: true,
				filters: [{ name: 'Audio', extensions: [...AUDIO_EXTENSIONS] }]
			});
			if (!result) return;
			const paths = Array.isArray(result) ? result : [result];
			libraryStore.importFiles(paths);
			libraryStore.setSource('audio');
		} catch (e) {
			notify(`Erreur ouverture fichier : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function browseFolder() {
		try {
			const result = await openDialog({ directory: true, multiple: false });
			if (!result || Array.isArray(result)) return;
			await libraryStore.addFolder(result);
			libraryStore.setSource('audio');
			notify('Indexation du dossier…', 'info');
		} catch (e) {
			notify(`Erreur dossier : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function insertAsset(asset: LibraryAsset) {
		const ok = await insertLibraryAsset({
			asset,
			trackId: insertTrackId
		});
		if (ok) onInserted?.();
	}

	function onRowClick(asset: LibraryAsset) {
		if (suppressNextClick) {
			suppressNextClick = false;
			return;
		}
		libraryStore.select(asset.id);
		keyboardFocus.setLibraryResults();
	}

	function onRowDblClick(asset: LibraryAsset) {
		void insertAsset(asset);
	}

	function onRowPointerEnter(asset: LibraryAsset) {
		if (rowPointerDrag) return;
		libraryStore.select(asset.id);
		libraryStore.scheduleHoverPreview(asset.id);
	}

	function onRowPointerLeave() {
		if (rowPointerDrag) return;
		libraryStore.stopPreview();
	}

	let rowPointerDrag: {
		pointerId: number;
		asset: LibraryAsset;
		startX: number;
		startY: number;
		active: boolean;
	} | null = null;
	let suppressNextClick = false;

	function onRowPointerDown(e: PointerEvent, asset: LibraryAsset) {
		if (e.button !== 0) return;
		const target = e.target as HTMLElement | null;
		if (target?.closest('button')) return;
		rowPointerDrag = {
			pointerId: e.pointerId,
			asset,
			startX: e.clientX,
			startY: e.clientY,
			active: false
		};
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
	}

	function onRowPointerMove(e: PointerEvent) {
		const drag = rowPointerDrag;
		if (!drag || e.pointerId !== drag.pointerId) return;
		const dx = e.clientX - drag.startX;
		const dy = e.clientY - drag.startY;
		if (!drag.active) {
			if (Math.hypot(dx, dy) < ARRANGE_POINTER_DRAG_THRESHOLD) return;
			drag.active = true;
			libraryStore.stopPreview();
			const durationSec = isMidiLibraryKind(drag.asset.kind)
				? libraryAssetLengthSec(drag.asset, $tempo.bpm)
				: (drag.asset.durationSec ?? null);
			beginLibraryDrag({
				path: drag.asset.path,
				source: 'library',
				durationSec,
				assetId: drag.asset.id,
				kind: drag.asset.kind,
				lengthBeats: drag.asset.lengthBeats ?? null
			});
			document.body.classList.add('library-file-dragging');
			keyboardFocus.setLibraryResults();
		}
		moveAudioFileDragPointer(e.clientX, e.clientY);
	}

	function finishRowPointerDrag(e: PointerEvent, commit: boolean) {
		const drag = rowPointerDrag;
		if (!drag || e.pointerId !== drag.pointerId) return;
		rowPointerDrag = null;
		try {
			(e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
		} catch {
			/* already released */
		}
		document.body.classList.remove('library-file-dragging');
		if (!drag.active) {
			endLibraryDrag();
			return;
		}
		suppressNextClick = true;
		if (commit) {
			requestAudioFileDragDrop(e.clientX, e.clientY);
			onInserted?.();
		} else {
			endLibraryDrag();
		}
	}

	function onRowPointerUp(e: PointerEvent) {
		finishRowPointerDrag(e, true);
	}

	function onRowPointerCancel(e: PointerEvent) {
		finishRowPointerDrag(e, false);
	}

	async function previewAsset(asset: LibraryAsset) {
		libraryStore.select(asset.id);
		keyboardFocus.setLibraryResults();
		try {
			await libraryStore.playPreview(asset.id);
		} catch (err) {
			notify(`Aperçu : ${(err as Error)?.message ?? err}`, 'error');
		}
	}

	const EMPTY_LIST_KEYS: Partial<Record<LibrarySourceId, string>> = {
		recents: 'library.noRecent',
		favorites: 'library.starToPin',
		chords: 'library.noFactoryChords',
		midi: 'library.noMidiPatterns',
		drums: 'library.noDrumPatterns'
	};

	$: emptyListKey = $libraryStore.query
		? 'library.noResults'
		: (EMPTY_LIST_KEYS[$libraryStore.source] ?? 'library.noAudioIndexed');

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
			const asset = visible.find((a) => a.id === $libraryStore.selectedId);
			if (asset) void insertAsset(asset);
			return;
		}
		if (e.key === ' ' && $libraryStore.selectedId) {
			e.preventDefault();
			void libraryStore.togglePreview($libraryStore.selectedId).catch((err) => {
				notify(`Aperçu : ${(err as Error)?.message ?? err}`, 'error');
			});
			return;
		}
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
			if ($libraryStore.query) {
				libraryStore.clearQuery();
				return;
			}
			if (variant === 'popup' && onRequestClose) {
				onRequestClose();
				return;
			}
			libraryStore.stopPreview();
		}
	}

	function onSearchInput(e: Event) {
		libraryStore.setQuery((e.target as HTMLInputElement).value);
	}

	function onSearchKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
			if ($libraryStore.query) {
				libraryStore.clearQuery();
				return;
			}
			if (variant === 'popup' && onRequestClose) {
				onRequestClose();
				return;
			}
			searchInputEl?.blur();
			resultsEl?.focus();
			keyboardFocus.setLibraryResults();
			return;
		}
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			resultsEl?.focus();
			keyboardFocus.setLibraryResults();
			moveSelection(1);
		}
	}

	function setSource(id: LibrarySourceId) {
		libraryStore.setSource(id);
		keyboardFocus.setLibraryResults();
	}
</script>

<div class="browser" class:browser--popup={variant === 'popup'}>
	<div class="chrome">
		<span class="chrome-title">{$t('library.title')}</span>
		{#if $libraryStore.indexer === 'scanning'}
			<span class="chrome-status">
				{$t('library.indexing')}{$libraryStore.indexerProgress != null
					? ` ${Math.round($libraryStore.indexerProgress * 100)}%`
					: ''}
			</span>
		{:else if $libraryStore.indexer === 'error'}
			<span class="chrome-status chrome-status--error" title={$libraryStore.indexerError ?? ''}>
				{$t('library.indexError')}
			</span>
		{/if}
		{#if variant === 'popup' && onRequestClose}
			<button
				type="button"
				class="close-btn"
				title={$t('common.closeEsc')}
				on:click={onRequestClose}>×</button
			>
		{/if}
	</div>

	<div class="search-row">
		<SearchInput
			placeholder={$t('common.search')}
			value={$libraryStore.query}
			bind:element={searchInputEl}
			on:input={onSearchInput}
			on:focus={() => keyboardFocus.setLibrarySearch()}
			on:keydown={onSearchKeydown}
			on:clear={() => libraryStore.clearQuery()}
		/>
		<button class="add-btn" type="button" on:click={browseFiles} title={$t('library.importFile')}>
			<svg width="10" height="10" viewBox="0 0 10 10" fill="none">
				<path d="M5 1V9M1 5H9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
			</svg>
		</button>
	</div>

	<nav class="sources" aria-label={$t('library.sourcesAria')}>
		{#each LIBRARY_SOURCE_ORDER as sourceId}
			<button
				type="button"
				class="source-btn"
				class:active={$libraryStore.source === sourceId}
				on:click={() => setSource(sourceId)}
			>
				{$t(`library.sources.${sourceId}`)}
			</button>
		{/each}
		<button type="button" class="source-btn source-btn--action" on:click={browseFolder}>
			{$t('library.addFolder')}
		</button>
	</nav>

	<div class="body">
		<div class="results-col">
			{#if visible.length === 0 && $libraryStore.source === 'audio' && !$libraryHasIndexedContent}
				<LibraryEmptyState title={$t('library.noSounds')} tip={$t('library.emptyTip')}>
					<button type="button" class="empty-btn" on:click={browseFolder}
						>{$t('library.addFolder')}</button
					>
					<button type="button" class="empty-btn empty-btn--ghost" on:click={browseFiles}
						>{$t('library.importFile')}</button
					>
				</LibraryEmptyState>
			{:else if visible.length === 0}
				<LibraryEmptyState title={$t('library.noResults')} tip={$t('library.noResultsTip')} />
			{:else}
				{#if $libraryStore.query}
					<div class="results-meta">
						{$t(visible.length === 1 ? 'library.resultOne' : 'library.resultMany', {
							values: {
								source: $t(`library.sources.${$libraryStore.source}`),
								n: visible.length
							}
						})}
					</div>
				{/if}

				<div
					class="file-list"
					bind:this={resultsEl}
					bind:clientHeight={viewportHeight}
					tabindex="0"
					role="listbox"
					aria-label={$t('library.resultsAria')}
					on:scroll={() => (scrollTop = resultsEl?.scrollTop ?? 0)}
					on:focus={() => keyboardFocus.setLibraryResults()}
					on:keydown={onResultsKeydown}
					on:mousedown={() => keyboardFocus.setLibraryResults()}
				>
					<div
						class="file-list-rows"
						role="presentation"
						style="padding-top: {LIST_PADDING +
							firstRow * ROW_HEIGHT}px; padding-bottom: {LIST_PADDING +
							(visible.length - lastRow) * ROW_HEIGHT}px"
					>
						{#each renderedRows as asset (asset.id)}
							{@const isImporting =
								$audioImportProgress.activeCount > 0 &&
								$audioImportProgress.filePath != null &&
								$audioImportProgress.filePath.replace(/\\/g, '/').toLowerCase() ===
									asset.path.replace(/\\/g, '/').toLowerCase()}
							<LibraryAssetRow
								{asset}
								selected={$libraryStore.selectedId === asset.id}
								previewing={$libraryStore.previewingId === asset.id}
								importing={isImporting}
								dragging={rowPointerDrag?.active === true && rowPointerDrag.asset.id === asset.id}
								favorite={$libraryStore.favoriteIds.includes(asset.id)}
								on:click={() => onRowClick(asset)}
								on:keydown={(e) => {
									if (e.key === 'Enter' || e.key === ' ') {
										e.preventDefault();
										onRowClick(asset);
									}
								}}
								on:dblclick={() => {
									if (!isImporting) onRowDblClick(asset);
								}}
								on:pointerenter={() => onRowPointerEnter(asset)}
								on:pointerleave={onRowPointerLeave}
								on:pointerdown={(e) => {
									if (!isImporting) onRowPointerDown(e, asset);
								}}
								on:pointermove={onRowPointerMove}
								on:pointerup={onRowPointerUp}
								on:pointercancel={onRowPointerCancel}
								on:favorite={() => libraryStore.toggleFavorite(asset.id)}
								on:preview={() => previewAsset(asset)}
								on:insert={() => insertAsset(asset)}
								on:cancelImport={() => cancelAudioImport(asset.path)}
							/>
						{:else}
							<LibraryEmptyState compact title={$t(emptyListKey)} />
						{/each}
					</div>
				</div>

				<LibraryDetail asset={selected} />
			{/if}
		</div>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.browser {
		flex: 1;
		min-width: 0;
		min-height: 0;
		display: flex;
		flex-direction: column;
		background: $panel-background-color;
		overflow: hidden;

		&--popup {
			background: $surface-overlay;
		}
	}

	.chrome {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: $space-4;
		padding: $space-4 $panel-inset 0;
		flex-shrink: 0;
	}

	.chrome-title {
		font-size: 10px;
		font-weight: 700;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: $text-muted;
	}

	.chrome-status {
		font-size: 9px;
		font-family: $font-mono;
		color: $text-faint;
		display: inline-flex;
		align-items: center;
		gap: $space-3;
		min-width: 0;
		margin-right: auto;

		&--error {
			color: $color-error;
		}
	}

	.close-btn {
		@include icon-button;
		font-size: 16px;
	}

	.search-row {
		display: flex;
		align-items: center;
		gap: $space-3;
		padding: $space-4 $panel-inset $space-3;
		flex-shrink: 0;
	}

	.add-btn {
		@include button-square($ctrl-height-sm);
	}

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

		&--action {
			margin-left: auto;
			color: $accent-text;
		}
	}

	.body {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}

	.results-col {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		min-height: 0;
	}

	.file-list {
		flex: 1;
		overflow-y: auto;
		overflow-x: hidden;
		overflow-anchor: none;
		outline: none;

		&:focus-visible {
			box-shadow: inset 0 0 0 1px $accent-border;
		}
	}

	.results-meta {
		font-size: 9px;
		font-family: $font-mono;
		color: $text-faint;
		padding: $space-2 $panel-inset $space-3;
	}

	:global(body.library-file-dragging) {
		cursor: grabbing !important;
		user-select: none !important;
	}

	:global(body.library-file-dragging *) {
		cursor: grabbing !important;
	}

	:global(body.library-file-dragging.library-file-dragging--rejected),
	:global(body.library-file-dragging.library-file-dragging--rejected *) {
		cursor: default !important;
	}

	.empty-btn {
		@include button($ctrl-height-sm, 11px);
		@include control-on;

		&--ghost {
			background: $surface-2;
			border-color: $border-subtle;
			color: $text-secondary;

			&:not(:disabled):hover {
				background: $surface-3;
				border-color: $border-strong;
				color: $text-color;
			}
		}
	}
</style>
