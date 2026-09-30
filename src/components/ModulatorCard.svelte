<!-- ModulatorCard.svelte — shared shell of a modulator: header, drag-to-assign handle, body slot -->
<script lang="ts">
	import { createEventDispatcher, onDestroy } from 'svelte';
	import { modulationDrag } from '../stores/modulationDrag';

	export let modulatorId: string;
	export let label: string;
	export let color: string;
	/** SVG path drawn in a 14 × 8 box. */
	export let icon: string;
	export let pickerOpen = false;

	const dispatch = createEventDispatcher<{ assign: string; togglepicker: void; remove: void }>();

	const DRAG_THRESHOLD_PX = 4;

	let dragStart: { x: number; y: number } | null = null;
	let ghost: { x: number; y: number } | null = null;

	function targetKeyAt(x: number, y: number): string | null {
		const el = document.elementFromPoint(x, y)?.closest<HTMLElement>('[data-mod-target]');
		return el?.dataset.modTarget ?? null;
	}

	function handleDragStart(e: PointerEvent) {
		if (e.button !== 0) return;
		e.preventDefault();
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		dragStart = { x: e.clientX, y: e.clientY };
	}

	function handleDragMove(e: PointerEvent) {
		if (!dragStart) return;
		if (!ghost) {
			if (Math.hypot(e.clientX - dragStart.x, e.clientY - dragStart.y) < DRAG_THRESHOLD_PX) return;
			document.body.style.cursor = 'grabbing';
			modulationDrag.set({ modulatorId, color, hoverKey: null });
		}
		ghost = { x: e.clientX, y: e.clientY };
		const hoverKey = targetKeyAt(e.clientX, e.clientY);
		modulationDrag.update((d) => (d && d.hoverKey !== hoverKey ? { ...d, hoverKey } : d));
	}

	function handleDragEnd(e: PointerEvent) {
		const key = ghost ? targetKeyAt(e.clientX, e.clientY) : null;
		endDrag();
		if (key) dispatch('assign', key);
	}

	function endDrag() {
		if (ghost) {
			document.body.style.cursor = '';
			modulationDrag.set(null);
		}
		dragStart = null;
		ghost = null;
	}

	onDestroy(endDrag);
</script>

<div class="card" style:--mod-color={color}>
	<div class="head">
		<div
			class="handle"
			class:dragging={ghost !== null}
			title="Glisser sur un réglage pour l'assigner"
			on:pointerdown={handleDragStart}
			on:pointermove={handleDragMove}
			on:pointerup={handleDragEnd}
			on:pointercancel={endDrag}
		>
			<svg width="14" height="8" viewBox="-1 -1 16 10" aria-hidden="true"><path d={icon} /></svg>
		</div>
		<span class="name">{label}</span>
		<button
			class="assign-btn"
			type="button"
			class:active={pickerOpen}
			on:mousedown|stopPropagation
			on:click|stopPropagation={() => dispatch('togglepicker')}
			title="Choisir un réglage dans la liste">Assigner</button
		>
		<button
			class="rm-btn"
			type="button"
			on:click|stopPropagation={() => dispatch('remove')}
			title="Supprimer">×</button
		>
	</div>

	<slot />
</div>

{#if ghost}
	<div
		class="drag-ghost"
		style:left="{ghost.x + 12}px"
		style:top="{ghost.y + 12}px"
		style:background={color}
	>
		{label}
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.card {
		padding: 5px 6px 6px 4px;
		border-left: 2px solid var(--mod-color);
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.head {
		display: flex;
		align-items: center;
		gap: 5px;
	}

	.handle {
		flex-shrink: 0;
		width: 20px;
		height: 16px;
		display: flex;
		align-items: center;
		justify-content: center;
		border-radius: 3px;
		cursor: grab;
		touch-action: none;
		color: var(--mod-color);
		&:hover {
			background: color-mix(in srgb, var(--mod-color) 18%, transparent);
		}
		&.dragging {
			cursor: grabbing;
			opacity: 0.5;
		}
	}

	.name {
		flex: 1;
		min-width: 0;
		font-size: 10px;
		font-weight: 600;
		color: var(--mod-color);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		user-select: none;
	}

	.assign-btn {
		flex-shrink: 0;
		padding: 1px 5px;
		border: 1px solid $border-subtle;
		border-radius: 3px;
		background: transparent;
		color: $text-subtle;
		font-size: 9px;
		line-height: 1.3;
		cursor: pointer;
		&:hover,
		&.active {
			background: color-mix(in srgb, var(--mod-color) 15%, transparent);
			color: $text-color;
			border-color: color-mix(in srgb, var(--mod-color) 45%, transparent);
		}
	}

	.rm-btn {
		width: 16px;
		height: 16px;
		border: none;
		background: transparent;
		color: $text-faint;
		font-size: 14px;
		line-height: 1;
		cursor: pointer;
		padding: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		flex-shrink: 0;
		&:hover {
			color: #ff4136;
		}
	}

	.drag-ghost {
		position: fixed;
		z-index: 1000;
		pointer-events: none;
		padding: 2px 7px;
		border-radius: 10px;
		font-size: 9px;
		font-family: $font-mono;
		font-weight: 700;
		color: #fff;
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.5);
		white-space: nowrap;
	}

	// Building blocks shared by every modulator body (slotted content).
	.card :global(svg path.icon),
	.handle path {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.3;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.card :global(.mod-display) {
		position: relative;
		height: 36px;
		border-radius: 4px;
		background: rgba(0, 0, 0, 0.3);
		border: 1px solid $border-faint;
		overflow: hidden;
	}

	.card :global(.mod-display > svg) {
		width: 100%;
		height: 100%;
		display: block;
	}

	.card :global(.mod-display .axis) {
		stroke: $border-faint;
		stroke-width: 1;
		stroke-dasharray: 2 2;
		vector-effect: non-scaling-stroke;
	}

	.card :global(.mod-display .curve) {
		fill: none;
		stroke: var(--mod-color);
		stroke-width: 1.5;
		stroke-linejoin: round;
		vector-effect: non-scaling-stroke;
	}

	.card :global(.mod-readout) {
		position: absolute;
		top: 2px;
		right: 4px;
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		color: $text-subtle;
		text-shadow: 0 0 3px rgba(0, 0, 0, 0.8);
		pointer-events: none;
	}

	.card :global(.mod-controls) {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: 4px;
	}

	.card :global(.mod-toggle) {
		flex-shrink: 0;
		height: 18px;
		padding: 0 4px;
		display: flex;
		align-items: center;
		justify-content: center;
		border: 1px solid $border-faint;
		border-radius: 3px;
		background: transparent;
		color: $text-faint;
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		cursor: pointer;
	}

	.card :global(.mod-toggle:hover) {
		color: $text-color;
		border-color: $border-subtle;
	}

	.card :global(.mod-toggle.active) {
		color: var(--mod-color);
		border-color: color-mix(in srgb, var(--mod-color) 55%, transparent);
		background: color-mix(in srgb, var(--mod-color) 14%, transparent);
	}

	.card :global(.mod-hint) {
		margin: 0;
		font-size: 8px;
		line-height: 1.35;
		color: $text-faint;
	}
</style>
