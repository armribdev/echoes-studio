<script lang="ts">
	import { rightPanel } from '../stores/rightPanel';
	import { selectedTrack } from '../stores/selectedTrack';
	import { transport } from '../stores/transport';
	import { projectEventStore } from '../shared/project/eventStore';
	import { addClip } from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';
	import { invoke } from '@tauri-apps/api/core';
	import { open as openDialog } from '@tauri-apps/plugin-dialog';
	import PanelResizeHandle from './PanelResizeHandle.svelte';
	import { persistProjectUiLayout } from '../lib/projectUiLayout';

	let isResizing = false;
	let resizeStartW = 0;

	function onPanelResizestart() {
		resizeStartW = $rightPanel.width;
	}

	function onPanelResize(e: CustomEvent<number>) {
		rightPanel.setWidth(resizeStartW + e.detail);
	}

	function onPanelResizeend() {
		persistProjectUiLayout();
	}

	let searchQuery = '';
	let libraryFiles: string[] = [];

	$: projectFiles = (() => {
		const seen = new Set<string>();
		for (const track of $projectEventStore.tracks) {
			for (const clip of track.clips) {
				if (clip.file_path) seen.add(clip.file_path);
			}
		}
		return [...seen];
	})();

	$: allFiles = [...new Set([...projectFiles, ...libraryFiles])];

	$: filteredProject = projectFiles.filter((f) =>
		fileName(f).toLowerCase().includes(searchQuery.toLowerCase())
	);

	$: filteredLibrary = libraryFiles.filter(
		(f) => !projectFiles.includes(f) && fileName(f).toLowerCase().includes(searchQuery.toLowerCase())
	);

	function fileName(path: string): string {
		return path.split(/[\\/]/).pop() ?? path;
	}

	function fileExt(path: string): string {
		return (path.split('.').pop() ?? '').toUpperCase();
	}

	async function browseFile() {
		try {
			const result = await openDialog({
				multiple: true,
				filters: [{ name: 'Audio', extensions: ['wav', 'mp3', 'flac', 'ogg', 'oga'] }],
			});
			if (!result) return;
			const paths = Array.isArray(result) ? result : [result];
			for (const p of paths) {
				if (!libraryFiles.includes(p) && !projectFiles.includes(p)) {
					libraryFiles = [...libraryFiles, p];
				}
			}
		} catch (e) {
			notify(`Erreur ouverture fichier : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function insertFile(filePath: string) {
		const targetTrackId = $selectedTrack.trackId ?? $projectEventStore.tracks[0]?.id;
		if (!targetTrackId) {
			notify('Créez une piste avant d\'importer.', 'error');
			return;
		}
		try {
			const dur = await invoke<number>('get_audio_clip_duration', { filePath });
			const startSec = $transport.position;
			await addClip(targetTrackId, filePath, startSec, startSec + dur);
			notify(`Importé : ${fileName(filePath)}`, 'success');
		} catch (e) {
			notify(`Erreur import : ${(e as Error)?.message ?? e}`, 'error');
		}
	}
</script>

<div
	class="library"
	class:open={$rightPanel.open}
	class:resizing={isResizing}
	style="--w: {$rightPanel.width}px"
>
	{#if $rightPanel.open}
		<PanelResizeHandle
			axis="horizontal"
			side="start"
			bind:resizing={isResizing}
			on:resizestart={onPanelResizestart}
			on:resize={onPanelResize}
			on:resizeend={onPanelResizeend}
		/>

		<div class="library-inner">
			<div class="search-row">
				<div class="search-wrap">
					<svg class="search-icon" width="11" height="11" viewBox="0 0 11 11" fill="none">
						<circle cx="4.5" cy="4.5" r="3.5" stroke="currentColor" stroke-width="1.2"/>
						<path d="M7.5 7.5L10 10" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
					</svg>
					<input
						class="search-input"
						type="text"
						placeholder="Rechercher…"
						bind:value={searchQuery}
					/>
					{#if searchQuery}
						<button class="search-clear" type="button" on:click={() => (searchQuery = '')}>
							<svg width="8" height="8" viewBox="0 0 8 8" fill="none">
								<path d="M1 1L7 7M7 1L1 7" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
							</svg>
						</button>
					{/if}
				</div>
				<button class="add-btn" type="button" on:click={browseFile} title="Ajouter des fichiers">
					<svg width="10" height="10" viewBox="0 0 10 10" fill="none">
						<path d="M5 1V9M1 5H9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
					</svg>
				</button>
			</div>

			<div class="file-list">
				{#if filteredProject.length > 0}
					<div class="section-label">CE PROJET</div>
					{#each filteredProject as filePath (filePath)}
						<!-- svelte-ignore a11y-no-static-element-interactions -->
						<div class="file-item" on:dblclick={() => insertFile(filePath)} title={filePath}>
							<span class="file-ext">{fileExt(filePath)}</span>
							<span class="file-name">{fileName(filePath)}</span>
							<button class="insert-btn" type="button" on:click={() => insertFile(filePath)} title="Ajouter à la piste">
								<svg width="8" height="8" viewBox="0 0 8 8" fill="none">
									<path d="M4 1V6M1.5 5.5L4 7.5L6.5 5.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
								</svg>
							</button>
						</div>
					{/each}
				{/if}

				{#if filteredLibrary.length > 0}
					<div class="section-label">BIBLIOTHÈQUE</div>
					{#each filteredLibrary as filePath (filePath)}
						<!-- svelte-ignore a11y-no-static-element-interactions -->
						<div class="file-item" on:dblclick={() => insertFile(filePath)} title={filePath}>
							<span class="file-ext">{fileExt(filePath)}</span>
							<span class="file-name">{fileName(filePath)}</span>
							<button class="insert-btn" type="button" on:click={() => insertFile(filePath)} title="Ajouter à la piste">
								<svg width="8" height="8" viewBox="0 0 8 8" fill="none">
									<path d="M4 1V6M1.5 5.5L4 7.5L6.5 5.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
								</svg>
							</button>
						</div>
					{/each}
				{/if}

				{#if filteredProject.length === 0 && filteredLibrary.length === 0}
					<div class="empty-state">
						<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round">
							<path d="M9 18V5l12-2v13"/>
							<circle cx="6" cy="18" r="3"/>
							<circle cx="18" cy="16" r="3"/>
						</svg>
						{#if searchQuery}
							<p>Aucun résultat pour « {searchQuery} »</p>
						{:else}
							<p>Cliquez sur <strong>+</strong> pour ajouter des fichiers audio à la bibliothèque</p>
						{/if}
					</div>
				{/if}
			</div>
		</div>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.library {
		width: 0;
		flex-shrink: 0;
		display: flex;
		flex-direction: row;
		overflow: hidden;
		transition: width 0.18s ease;
		position: relative;
		border-left: 1px solid $border-color;

		&.open {
			width: var(--w);
		}

		&.resizing { user-select: none; transition: none; }
	}

	.library-inner {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		background: $panel-background-color;
		overflow: hidden;
		min-height: 0;
	}

	// ── Search bar ────────────────────────────────────────────────────────────
	.search-row {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 8px 8px 8px 12px;
		border-bottom: 1px solid $border-color;
		flex-shrink: 0;
	}

	.search-wrap {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 5px;
		background: rgba(255,255,255,0.05);
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

		&::placeholder { color: $text-faint; }
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

		&:hover { color: $text-color; }
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
		transition: background 0.12s, color 0.12s, border-color 0.12s;

		&:hover {
			background: rgba(21,113,255,0.12);
			color: $text-color;
			border-color: rgba(21,113,255,0.4);
		}
	}

	// ── File list ─────────────────────────────────────────────────────────────
	.file-list {
		flex: 1;
		overflow-y: auto;
		overflow-x: hidden;
		padding: 4px 0;
	}

	.section-label {
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		color: $text-faint;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		padding: 8px 10px 4px;
	}

	.file-item {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 4px 8px 4px 10px;
		cursor: pointer;
		transition: background 0.1s;

		&:hover {
			background: $surface-hover;

			.insert-btn { opacity: 1; }
		}
	}

	.file-ext {
		font-size: 7px;
		font-family: $font-mono;
		font-weight: 700;
		color: $accent-color;
		background: rgba(21,113,255,0.12);
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
		transition: color 0.1s, opacity 0.1s;

		&:hover { color: $accent-color; }
	}

	// ── Empty state ───────────────────────────────────────────────────────────
	.empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 10px;
		padding: 32px 16px;
		color: $text-faint;

		svg { opacity: 0.25; }

		p {
			font-size: 10px;
			text-align: center;
			margin: 0;
			line-height: 1.5;
			color: $text-faint;
		}

		strong { color: $text-subtle; }
	}
</style>
