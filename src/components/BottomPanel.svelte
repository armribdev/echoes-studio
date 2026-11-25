<script lang="ts">
	import { get } from 'svelte/store';
	import { bottomPanel } from '../stores/bottomPanel';
	import { closeEditor } from '../frontend/pianoRoll/clipEditor';
	import { previewPanelResizing, editPanelResizing } from '../frontend/pianoRoll/previewRefit';
	import { computePreviewResizeMax } from '../frontend/pianoRoll/previewLayout';
	import { pianoRollStore } from '../stores/pianoRoll';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import PianoRoll from './PianoRoll.svelte';
	import PanelResizeHandle from './PanelResizeHandle.svelte';
	import { persistProjectUiLayout } from '../lib/projectUiLayout';
	import { t } from 'svelte-i18n';

	let isResizing = false;
	let resizeStartH = 0;

	$: editMode = $keyboardFocus === 'piano-roll';

	function onPanelResizestart() {
		resizeStartH = $bottomPanel.height;
		if (editMode) editPanelResizing.set(true);
		else previewPanelResizing.set(true);
	}

	function onPanelResize(e: CustomEvent<number>) {
		const nextH = resizeStartH + e.detail;
		if (editMode) {
			bottomPanel.setEditHeight(nextH);
		} else {
			const notes = get(pianoRollStore).notes;
			bottomPanel.setPreviewHeight(nextH, computePreviewResizeMax(notes));
		}
	}

	function onPanelResizeend() {
		previewPanelResizing.set(false);
		editPanelResizing.set(false);
		persistProjectUiLayout();
	}
</script>

{#if $bottomPanel.open}
	<div
		class="bottom-panel"
		class:bottom-panel--preview={!editMode}
		style="height: {$bottomPanel.height}px"
		class:resizing={isResizing}
	>
		<div class="panel-body">
			{#if $pianoRollStore.open}
				<PianoRoll />
			{:else}
				<div class="empty-state">
					<button
						class="empty-close"
						on:click={closeEditor}
						title={$t('common.closeTab')}
						aria-label={$t('common.close')}
					>
						<svg width="10" height="10" viewBox="0 0 10 10" fill="none">
							<path
								d="M1.5 1.5L8.5 8.5M8.5 1.5L1.5 8.5"
								stroke="currentColor"
								stroke-width="1.5"
								stroke-linecap="round"
							/>
						</svg>
					</button>
					<svg
						width="28"
						height="28"
						viewBox="0 0 28 28"
						fill="none"
						stroke="currentColor"
						stroke-width="1.2"
						stroke-linecap="round"
					>
						<rect x="2" y="8" width="5" height="18" rx="1" />
						<rect x="9" y="2" width="5" height="24" rx="1" />
						<rect x="16" y="6" width="5" height="20" rx="1" />
						<rect x="23" y="4" width="3" height="22" rx="1" />
					</svg>
					<p>Sélectionnez un clip MIDI ou double-cliquez pour éditer</p>
				</div>
			{/if}
		</div>

		<PanelResizeHandle
			axis="vertical"
			side="start"
			bind:resizing={isResizing}
			on:resizestart={onPanelResizestart}
			on:resize={onPanelResize}
			on:resizeend={onPanelResizeend}
		/>
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.bottom-panel {
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		background: $background-color;
		border-top: 1px solid $border-color;
		overflow: hidden;
		position: relative;

		&.resizing {
			cursor: ns-resize;
			user-select: none;
		}

		&--preview {
			flex-shrink: 0;
		}
	}

	.panel-body {
		flex: 1;
		overflow: hidden;
		display: flex;
		flex-direction: column;
		min-height: 0;
		position: relative;
	}

	.empty-state {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: $space-5;
		height: 100%;
		color: $text-faint;
		position: relative;

		> svg {
			opacity: 0.3;
		}

		p {
			font-size: 11px;
			color: $text-faint;
			margin: 0;
			text-align: center;
		}
	}

	.empty-close {
		@include icon-button;
		@include icon-button-danger;
		position: absolute;
		top: 6px;
		right: 8px;
	}
</style>
