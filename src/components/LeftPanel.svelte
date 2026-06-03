<script lang="ts">
	import { leftPanel } from '../stores/leftPanel';
	import { selectedTrack } from '../stores/selectedTrack';
	import EffectChain from './EffectChain.svelte';
	import InstrumentPanel from './InstrumentPanel.svelte';
	import ModulationPanel from './ModulationPanel.svelte';
	import PanelResizeHandle from './PanelResizeHandle.svelte';

	let isResizing = false;
	let resizeStartW = 0;

	function onPanelResizestart() {
		resizeStartW = $leftPanel.width;
	}

	function onPanelResize(e: CustomEvent<number>) {
		leftPanel.setWidth(resizeStartW + e.detail);
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
					<InstrumentPanel
						trackId={$selectedTrack.trackId}
						instrument={$selectedTrack.instrument}
					/>
				</div>

				<div class="section-divider"></div>

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

		<PanelResizeHandle
			axis="horizontal"
			side="end"
			bind:resizing={isResizing}
			on:resizestart={onPanelResizestart}
			on:resize={onPanelResize}
		/>
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
		background: $panel-background-color;
		overflow-y: auto;
		overflow-x: hidden;
		min-height: 0;
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
