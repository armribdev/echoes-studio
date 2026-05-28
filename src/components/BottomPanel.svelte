<script lang="ts">
	import { bottomPanel } from '../stores/bottomPanel';
	import { pianoRollStore } from '../stores/pianoRoll';
	import PianoRoll from './PianoRoll.svelte';

	let isResizing = false;
	let resizeStartY = 0;
	let resizeStartH = 0;

	function onResizeStart(e: MouseEvent) {
		isResizing = true;
		resizeStartY = e.clientY;
		resizeStartH = $bottomPanel.height;
		window.addEventListener('mousemove', onResizeMove);
		window.addEventListener('mouseup', onResizeEnd);
	}

	function onResizeMove(e: MouseEvent) {
		const delta = resizeStartY - e.clientY;
		bottomPanel.setHeight(resizeStartH + delta);
	}

	function onResizeEnd() {
		isResizing = false;
		window.removeEventListener('mousemove', onResizeMove);
		window.removeEventListener('mouseup', onResizeEnd);
	}
</script>

{#if $bottomPanel.open}
<!-- svelte-ignore a11y-no-static-element-interactions -->
<div class="bottom-panel" style="height: {$bottomPanel.height}px" class:resizing={isResizing}>
	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div class="resize-handle" on:mousedown={onResizeStart}></div>

	<div class="panel-header">
		<div class="panel-title">
			<svg width="11" height="11" viewBox="0 0 11 11" fill="none">
				<rect x="0.5" y="3.5" width="2" height="7" rx="0.5" fill="currentColor"/>
				<rect x="3.5" y="0.5" width="2" height="10" rx="0.5" fill="currentColor"/>
				<rect x="6.5" y="2.5" width="2" height="8" rx="0.5" fill="currentColor"/>
				<rect x="9.5" y="1.5" width="1" height="9" rx="0.5" fill="currentColor"/>
			</svg>
			Piano Roll
			{#if $pianoRollStore.open && $pianoRollStore.trackName}
				<span class="track-badge" style="--c: {$pianoRollStore.trackColor}">
					<span class="track-dot"></span>
					<span class="track-label">{$pianoRollStore.trackName}</span>
				</span>
			{/if}
		</div>
		<button class="close-btn" on:click={() => bottomPanel.close()} title="Fermer">
			<svg width="10" height="10" viewBox="0 0 10 10" fill="none">
				<path d="M1.5 1.5L8.5 8.5M8.5 1.5L1.5 8.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
			</svg>
		</button>
	</div>

	<div class="panel-body">
		{#if $pianoRollStore.open}
			<PianoRoll />
		{:else}
			<div class="empty-state">
				<svg width="28" height="28" viewBox="0 0 28 28" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round">
					<rect x="2" y="8" width="5" height="18" rx="1"/>
					<rect x="9" y="2" width="5" height="24" rx="1"/>
					<rect x="16" y="6" width="5" height="20" rx="1"/>
					<rect x="23" y="4" width="3" height="22" rx="1"/>
				</svg>
				<p>Cliquez sur <kbd>⊞</kbd> d'une piste pour éditer ses notes MIDI</p>
			</div>
		{/if}
	</div>
</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.bottom-panel {
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		background: $background-color;
		border-top: 1px solid $border-color;
		overflow: hidden;
		position: relative;

		&.resizing { cursor: ns-resize; user-select: none; }
	}

	.resize-handle {
		position: absolute;
		top: 0; left: 0; right: 0;
		height: 4px;
		cursor: ns-resize;
		z-index: 10;
		transition: background 0.15s;

		&:hover  { background: rgba($accent-color, 0.4); }
		&:active { background: $accent-color; }
	}

	.panel-header {
		height: 30px;
		flex-shrink: 0;
		display: flex;
		align-items: center;
		padding: 0 8px 0 10px;
		background: $panel-background-color;
		border-bottom: 1px solid $border-color;
		gap: 8px;
	}

	.panel-title {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 11px;
		font-weight: 500;
		color: $text-muted;

		svg { flex-shrink: 0; }
	}

	.track-badge {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 0 6px;
		height: 18px;
		border-radius: $radius-sm;
		background: rgba(255,255,255,0.04);
		border: 1px solid $border-faint;
	}

	.track-dot {
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--c, #4d96ff);
		flex-shrink: 0;
	}

	.track-label {
		font-size: 10px;
		font-family: $font-mono;
		color: $text-subtle;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 100px;
	}

	.close-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 22px;
		height: 22px;
		padding: 0;
		background: transparent;
		border: none;
		border-radius: $radius-sm;
		color: $text-faint;
		cursor: pointer;
		flex-shrink: 0;
		transition: background 0.12s, color 0.12s;

		&:hover { background: rgba(255, 65, 54, 0.15); color: #ff4136; }
	}

	.panel-body {
		flex: 1;
		overflow: hidden;
		display: flex;
		flex-direction: column;
		min-height: 0;
	}

	.empty-state {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 10px;
		height: 100%;
		color: $text-faint;

		svg { opacity: 0.3; }

		p {
			font-size: 11px;
			color: $text-faint;
			margin: 0;
			text-align: center;
		}

		kbd {
			display: inline-flex;
			align-items: center;
			justify-content: center;
			width: 16px;
			height: 16px;
			background: $surface-1;
			border: 1px solid $border-subtle;
			border-radius: 3px;
			font-size: 10px;
			font-family: $font-mono;
			color: $text-subtle;
			vertical-align: middle;
		}
	}
</style>
