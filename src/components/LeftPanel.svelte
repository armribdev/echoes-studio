<script lang="ts">
	import { leftPanel } from '../stores/leftPanel';
	import { selectedTrack } from '../stores/selectedTrack';
	import EffectChain from './EffectChain.svelte';
	import ModulationPanel from './ModulationPanel.svelte';

	let isResizing = false;
	let resizeStartX = 0;
	let resizeStartW = 0;

	function onResizeStart(e: MouseEvent) {
		isResizing = true;
		resizeStartX = e.clientX;
		resizeStartW = $leftPanel.width;
		window.addEventListener('mousemove', onResizeMove);
		window.addEventListener('mouseup', onResizeEnd);
	}

	function onResizeMove(e: MouseEvent) {
		const delta = e.clientX - resizeStartX;
		leftPanel.setWidth(resizeStartW + delta);
	}

	function onResizeEnd() {
		isResizing = false;
		window.removeEventListener('mousemove', onResizeMove);
		window.removeEventListener('mouseup', onResizeEnd);
	}
</script>

<div
	class="inspector"
	class:open={$leftPanel.open}
	class:resizing={isResizing}
	style="--w: {$leftPanel.width}px"
>
	{#if $leftPanel.open}
		<div class="inspector-inner">
			{#if $selectedTrack.trackId}
				<div class="track-header" style="--c: {$selectedTrack.trackColor}">
					<span class="track-color-bar"></span>
					<span class="track-title">{$selectedTrack.trackName || 'Piste sans nom'}</span>
				</div>

				<div class="section">
					<EffectChain
						trackId={$selectedTrack.trackId}
						effects={$selectedTrack.effects}
					/>
				</div>

				<div class="section-divider"></div>

				<div class="section">
					<ModulationPanel
						trackId={$selectedTrack.trackId}
						modulators={$selectedTrack.modulators}
						mod_routes={$selectedTrack.mod_routes}
						effects={$selectedTrack.effects}
					/>
				</div>
			{/if}
		</div>

		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<div class="resize-handle" on:mousedown={onResizeStart}></div>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.inspector {
		width: 0;
		flex-shrink: 0;
		display: flex;
		flex-direction: row;
		overflow: hidden;
		transition: width 0.18s ease;
		position: relative;
		border-right: 1px solid $border-color;

		&.open {
			width: var(--w);
		}

		&.resizing { user-select: none; transition: none; }
	}

	.inspector-inner {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		background: $pannel-background-color;
		overflow-y: auto;
		overflow-x: hidden;
		min-height: 0;
	}

	.resize-handle {
		position: absolute;
		top: 0;
		right: 0;
		width: 4px;
		height: 100%;
		cursor: ew-resize;
		z-index: 10;
		transition: background 0.15s;

		&:hover  { background: rgba($accent-color, 0.4); }
		&:active { background: $accent-color; }
	}

	// ── Track header ──────────────────────────────────────────────────────────
	.track-header {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 10px 10px 10px 0;
		border-bottom: 1px solid $border-color;
		flex-shrink: 0;
	}

	.track-color-bar {
		width: 3px;
		height: 28px;
		background: var(--c, $accent-color);
		border-radius: 0 2px 2px 0;
		flex-shrink: 0;
	}

	.track-title {
		font-size: 12px;
		font-weight: 600;
		color: $text-color;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	// ── Sections ──────────────────────────────────────────────────────────────
	.section {
		flex-shrink: 0;
	}

	.section-divider {
		height: 1px;
		background: $border-color;
		flex-shrink: 0;
	}

</style>
