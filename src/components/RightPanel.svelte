<script lang="ts">
	import { onMount } from 'svelte';
	import { rightPanel } from '../stores/rightPanel';
	import PanelResizeHandle from './PanelResizeHandle.svelte';
	import LibraryBrowser from './LibraryBrowser.svelte';
	import { persistProjectUiLayout } from '../lib/projectUiLayout';
	import { libraryStore } from '../frontend/library';

	let isResizing = false;
	let resizeStartW = 0;

	$: if (!$rightPanel.open) {
		libraryStore.stopPreview();
	}

	onMount(() => {
		libraryStore.hydrateFromPersistence();
		void libraryStore.rescanIndexedFolders();
	});

	function onPanelResizestart() {
		resizeStartW = $rightPanel.width;
	}

	function onPanelResize(e: CustomEvent<number>) {
		rightPanel.setWidth(resizeStartW + e.detail);
	}

	function onPanelResizeend() {
		persistProjectUiLayout();
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
		<LibraryBrowser variant="dock" autofocus={$rightPanel.open} />
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

		&.resizing {
			user-select: none;
			transition: none;
		}
	}
</style>
