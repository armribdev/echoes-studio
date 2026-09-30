<script lang="ts">
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
		formatDuration,
		formatBeats,
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

	/** Docked panel vs Bitwig-style pop-up (same browser, different shell). */
	export let variant: 'dock' | 'popup' = 'dock';
	/** Focus search when mounted / when the shared focus tick fires. */
	export let autofocus = false;
	/** Target track for Enter/insert; `null` creates a track if needed. */
	export let insertTrackId: string | null | undefined = undefined;
	export let onInserted: (() => void) | undefined = undefined;
	/** Pop-up: Esc with empty query (or close request). */
	export let onRequestClose: (() => void) | undefined = undefined;

	let searchInputEl: HTMLInputElement | null = null;
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

	function onFavoriteClick(e: MouseEvent, asset: LibraryAsset) {
		e.stopPropagation();
		libraryStore.toggleFavorite(asset.id);
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
			<button type="button" class="close-btn" title={$t('common.closeEsc')} on:click={onRequestClose}
				>×</button
			>
		{/if}
	</div>

	<div class="search-row">
		<div class="search-wrap">
			<svg class="search-icon" width="11" height="11" viewBox="0 0 11 11" fill="none">
				<circle cx="4.5" cy="4.5" r="3.5" stroke="currentColor" stroke-width="1.2" />
				<path d="M7.5 7.5L10 10" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
			</svg>
			<input
				class="search-input"
				type="text"
				placeholder={$t('common.search')}
				value={$libraryStore.query}
				bind:this={searchInputEl}
				on:input={(e) => libraryStore.setQuery(e.currentTarget.value)}
				on:focus={() => keyboardFocus.setLibrarySearch()}
				on:keydown={onSearchKeydown}
			/>
			{#if $libraryStore.query}
				<button class="search-clear" type="button" on:click={() => libraryStore.clearQuery()}>
					<svg width="8" height="8" viewBox="0 0 8 8" fill="none">
						<path
							d="M1 1L7 7M7 1L1 7"
							stroke="currentColor"
							stroke-width="1.4"
							stroke-linecap="round"
						/>
					</svg>
				</button>
			{/if}
		</div>
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
				<div class="empty-state">
					<p class="empty-title">{$t('library.noSounds')}</p>
					<div class="empty-actions">
						<button type="button" class="empty-btn" on:click={browseFolder}
							>{$t('library.addFolder')}</button
						>
						<button type="button" class="empty-btn empty-btn--ghost" on:click={browseFiles}>
							{$t('library.importFile')}
						</button>
					</div>
					<p class="empty-tip">{$t('library.emptyTip')}</p>
				</div>
			{:else if visible.length === 0}
				<div class="empty-state">
					<p class="empty-title">{$t('library.noResults')}</p>
					<p class="empty-tip">{$t('library.noResultsTip')}</p>
				</div>
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
						<div
							class="file-item"
							class:selected={$libraryStore.selectedId === asset.id}
							class:previewing={$libraryStore.previewingId === asset.id}
							class:importing={isImporting}
							class:dragging={rowPointerDrag?.active && rowPointerDrag.asset.id === asset.id}
							role="option"
							tabindex="-1"
							aria-selected={$libraryStore.selectedId === asset.id}
							aria-busy={isImporting}
							title={asset.path}
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
						>
							<button
								class="fav-btn"
								type="button"
								class:on={$libraryStore.favoriteIds.includes(asset.id)}
								title={$t('library.favorite')}
								on:click={(e) => onFavoriteClick(e, asset)}
								on:pointerdown={(e) => e.stopPropagation()}
							>
								{$libraryStore.favoriteIds.includes(asset.id) ? '★' : '☆'}
							</button>
							<span class="file-ext">{asset.extension.toUpperCase()}</span>
							<span class="file-name">{asset.name}</span>
							{#if isImporting}
								<button
									class="cancel-import-btn"
									type="button"
									title={$t('library.cancelImport')}
									aria-label={$t('library.cancelImport')}
									on:pointerdown={(e) => e.stopPropagation()}
									on:click={(e) => {
										e.stopPropagation();
										void cancelAudioImport(asset.path);
									}}
								>
									<span class="cancel-import-ring" aria-hidden="true"></span>
									<span class="cancel-import-square" aria-hidden="true"></span>
								</button>
							{:else}
								{#if $libraryStore.previewingId !== asset.id}
									<button
										class="preview-btn"
										type="button"
										title={$t('library.preview')}
										on:pointerdown={(e) => e.stopPropagation()}
										on:click={(e) => onPreviewClick(e, asset)}
									>
										▶
									</button>
								{/if}
								<button
									class="insert-btn"
									type="button"
									title={$t('library.insertAtPlayhead')}
									on:pointerdown={(e) => e.stopPropagation()}
									on:click={() => insertAsset(asset)}
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
					{:else}
						<div class="empty-state empty-state--compact">
							{#if $libraryStore.query}
								<p>{$t('library.noResults')}</p>
							{:else if $libraryStore.source === 'recents'}
								<p>{$t('library.noRecent')}</p>
							{:else if $libraryStore.source === 'favorites'}
								<p>{$t('library.starToPin')}</p>
							{:else if $libraryStore.source === 'chords'}
								<p>{$t('library.noFactoryChords')}</p>
							{:else if $libraryStore.source === 'midi'}
								<p>{$t('library.noMidiPatterns')}</p>
							{:else if $libraryStore.source === 'drums'}
								<p>{$t('library.noDrumPatterns')}</p>
							{:else}
								<p>{$t('library.noAudioIndexed')}</p>
							{/if}
						</div>
					{/each}
				</div>

				<div class="detail">
					{#if selected}
						<div class="detail-name" title={selected.path}>{selected.name}</div>
						<div class="detail-meta">
							<span>{selected.extension.toUpperCase()}</span>
							{#if isMidiLibraryKind(selected.kind)}
								<span>{formatBeats(selected.lengthBeats)}</span>
								{#if selected.keyRoot}
									<span>{selected.keyRoot}</span>
								{/if}
							{:else}
								<span>{formatDuration(selected.durationSec)}</span>
							{/if}
						</div>
						{#if selected.kind === 'loop' || selected.kind === 'one_shot'}
							<div class="detail-tip">Placé à la longueur d'origine (pas de warp)</div>
						{/if}
					{:else}
						<div class="detail-placeholder">{$t('library.selectSound')}</div>
					{/if}
				</div>
			{/if}
		</div>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.browser {
		flex: 1;
		min-width: 0;
		min-height: 0;
		display: flex;
		flex-direction: column;
		background: $panel-background-color;
		overflow: hidden;

		&--popup {
			background: #26262e;
		}
	}

	.chrome {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		padding: 8px 10px 0;
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
		gap: 5px;
		min-width: 0;
		margin-right: auto;

		&--error {
			color: $color-error;
		}
	}

	.close-btn {
		width: 22px;
		height: 22px;
		border: none;
		border-radius: $radius-sm;
		background: transparent;
		color: $text-subtle;
		font-size: 16px;
		line-height: 1;
		cursor: pointer;

		&:hover {
			color: $text-color;
			background: $surface-hover;
		}
	}

	.search-row {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 8px 8px 6px 10px;
		flex-shrink: 0;
	}

	.search-wrap {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 5px;
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid $border-faint;
		border-radius: $radius-sm;
		padding: 0 6px;
		height: 24px;
	}

	.search-icon {
		flex-shrink: 0;
		color: $text-faint;
	}

	.search-input {
		flex: 1;
		background: transparent;
		border: none;
		outline: none;
		color: $text-color;
		font-size: 11px;
		font-family: inherit;
		padding: 0;
		min-width: 0;

		&::placeholder {
			color: $text-faint;
		}
	}

	.search-clear {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 14px;
		height: 14px;
		background: transparent;
		border: none;
		padding: 0;
		color: $text-faint;
		cursor: pointer;
		flex-shrink: 0;

		&:hover {
			color: $text-color;
		}
	}

	.add-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		background: transparent;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		color: $text-subtle;
		cursor: pointer;
		flex-shrink: 0;
		transition:
			background 0.12s,
			color 0.12s,
			border-color 0.12s;

		&:hover {
			background: rgba(21, 113, 255, 0.12);
			color: $text-color;
			border-color: rgba(21, 113, 255, 0.4);
		}
	}

	.sources {
		flex-shrink: 0;
		display: flex;
		flex-direction: row;
		align-items: center;
		gap: 2px;
		padding: 0 8px 6px 10px;
		overflow-x: auto;
		border-bottom: 1px solid $border-color;
	}

	.source-btn {
		flex-shrink: 0;
		text-align: center;
		background: transparent;
		border: none;
		border-radius: $radius-sm;
		color: $text-subtle;
		font-size: 10px;
		padding: 5px 8px;
		cursor: pointer;
		white-space: nowrap;

		&:hover {
			background: $surface-hover;
			color: $text-color;
		}

		&.active {
			background: rgba(21, 113, 255, 0.14);
			color: $text-color;
		}

		&--action {
			margin-left: auto;
			color: $accent-color;
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
		padding: 4px 0;
		outline: none;

		&:focus-visible {
			box-shadow: inset 0 0 0 1px rgba(21, 113, 255, 0.35);
		}
	}

	.results-meta {
		font-size: 9px;
		font-family: $font-mono;
		color: $text-faint;
		padding: 4px 10px 6px;
	}

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
		transition: background 0.1s;

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

	:global(body.library-file-dragging) {
		cursor: grabbing !important;
		user-select: none !important;
	}

	:global(body.library-file-dragging *) {
		cursor: grabbing !important;
	}

	:global(body.library-file-dragging.library-file-dragging--rejected),
	:global(body.library-file-dragging.library-file-dragging--rejected *) {
		cursor: not-allowed !important;
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
			color 0.1s,
			opacity 0.1s;
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

	.empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 12px;
		padding: 28px 14px;
		color: $text-faint;
		text-align: center;

		&--compact {
			padding: 24px 12px;
			gap: 6px;
		}
	}

	.empty-title {
		font-size: 12px;
		color: $text-subtle;
		margin: 0;
	}

	.empty-actions {
		display: flex;
		flex-direction: column;
		gap: 6px;
		width: 100%;
		max-width: 160px;
	}

	.empty-btn {
		height: 26px;
		border-radius: $radius-sm;
		border: 1px solid rgba(21, 113, 255, 0.45);
		background: rgba(21, 113, 255, 0.14);
		color: $text-color;
		font-size: 11px;
		cursor: pointer;

		&:hover {
			background: rgba(21, 113, 255, 0.22);
		}

		&--ghost {
			background: transparent;
			border-color: $border-subtle;
			color: $text-subtle;

			&:hover {
				color: $text-color;
				border-color: $border-color;
			}
		}
	}

	.empty-tip {
		font-size: 10px;
		line-height: 1.45;
		margin: 0;
		color: $text-faint;
	}
</style>
