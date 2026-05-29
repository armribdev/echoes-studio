<!-- Poignée de redimensionnement partagée (inspecteur, bibliothèque, panneau inférieur). -->
<script lang="ts">
	import { createEventDispatcher } from 'svelte';

	/** Axe du geste de redimensionnement. */
	export let axis: 'horizontal' | 'vertical';
	/** Bord du panneau : start = gauche/haut, end = droite. */
	export let side: 'start' | 'end';
	/** État de drag (bindable pour la classe .resizing du parent). */
	export let resizing = false;

	const dispatch = createEventDispatcher<{
		resizestart: void;
		resize: number;
		resizeend: void;
	}>();

	let originX = 0;
	let originY = 0;

	function deltaFromPointer(e: MouseEvent): number {
		if (axis === 'horizontal') {
			const dx = e.clientX - originX;
			return side === 'end' ? dx : -dx;
		}
		const dy = e.clientY - originY;
		return side === 'start' ? -dy : dy;
	}

	function onResizeMove(e: MouseEvent) {
		dispatch('resize', deltaFromPointer(e));
	}

	function onResizeEnd() {
		resizing = false;
		window.removeEventListener('mousemove', onResizeMove);
		window.removeEventListener('mouseup', onResizeEnd);
		dispatch('resizeend');
	}

	function onResizeStart(e: MouseEvent) {
		resizing = true;
		originX = e.clientX;
		originY = e.clientY;
		dispatch('resizestart');
		window.addEventListener('mousemove', onResizeMove);
		window.addEventListener('mouseup', onResizeEnd);
	}
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	class="resize-handle"
	class:resize-handle--horizontal={axis === 'horizontal'}
	class:resize-handle--vertical={axis === 'vertical'}
	class:resize-handle--start={side === 'start'}
	class:resize-handle--end={side === 'end'}
	on:mousedown={onResizeStart}
></div>

<style lang="scss">
	@use '../variables.scss' as *;

	.resize-handle {
		position: absolute;
		z-index: 10;
		transition: background 0.15s;

		&:hover  { background: rgba($accent-color, 0.4); }
		&:active { background: $accent-color; }
	}

	.resize-handle--horizontal {
		top: 0;
		width: 4px;
		height: 100%;
		cursor: ew-resize;

		&.resize-handle--start { left: 0; }
		&.resize-handle--end   { right: 0; }
	}

	.resize-handle--vertical {
		left: 0;
		right: 0;
		height: 4px;
		cursor: ns-resize;

		&.resize-handle--start { top: 0; }
		&.resize-handle--end   { bottom: 0; }
	}
</style>
