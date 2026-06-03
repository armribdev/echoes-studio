<script lang="ts">
	import { leftPanel } from '../stores/leftPanel';
	import { selectedTrack } from '../stores/selectedTrack';
	import EffectChain from './EffectChain.svelte';
	import InstrumentPanel from './InstrumentPanel.svelte';
	import ModulationPanel from './ModulationPanel.svelte';
	import PanelResizeHandle from './PanelResizeHandle.svelte';
	import { persistProjectUiLayout } from '../lib/projectUiLayout';

	let isResizing = false;
	let resizeStartW = 0;

	function onPanelResizestart() {
		resizeStartW = $leftPanel.width;
	}

	function onPanelResize(e: CustomEvent<number>) {
		leftPanel.setWidth(resizeStartW + e.detail);
	}

	function onPanelResizeend() {
		persistProjectUiLayout();
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
				<InstrumentPanel
					compact
					trackId={$selectedTrack.trackId}
					instrument={$selectedTrack.instrument}
					trackName={$selectedTrack.trackName}
					trackColor={$selectedTrack.trackColor}
				/>

				<div class="section-divider"></div>

				<EffectChain
					compact
					trackId={$selectedTrack.trackId}
					effects={$selectedTrack.effects}
				/>

				<div class="section-divider"></div>

				<ModulationPanel
					compact
					trackId={$selectedTrack.trackId}
					modulators={$selectedTrack.modulators}
					mod_routes={$selectedTrack.mod_routes}
					effects={$selectedTrack.effects}
				/>
			{/if}
		</div>

		<PanelResizeHandle
			axis="horizontal"
			side="end"
			bind:resizing={isResizing}
			on:resizestart={onPanelResizestart}
			on:resize={onPanelResize}
			on:resizeend={onPanelResizeend}
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

	.section-divider {
		height: 1px;
		background: $border-color;
		flex-shrink: 0;
	}

</style>
