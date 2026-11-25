<script lang="ts">
	import { createEventDispatcher } from 'svelte';

	/** Left edge in viewport pixels. */
	export let startPx: number;
	/** Right edge in viewport pixels. */
	export let endPx: number;

	const dispatch = createEventDispatcher<{
		dragstart: void;
		drag: { kind: 'start' | 'end'; originSec: number; currentSec: number; altKey: boolean };
		dragend: void;
	}>();

	export let secondsFromClientX: (clientX: number) => number;

	let drag: { kind: 'start' | 'end'; originSec: number } | null = null;

	function onPointerDown(event: PointerEvent, kind: 'start' | 'end') {
		event.stopPropagation();
		event.preventDefault();
		drag = { kind, originSec: secondsFromClientX(event.clientX) };
		dispatch('dragstart');
		window.addEventListener('pointermove', onPointerMove);
		window.addEventListener('pointerup', onPointerUp);
	}

	function onPointerMove(event: PointerEvent) {
		if (!drag) return;
		dispatch('drag', {
			kind: drag.kind,
			originSec: drag.originSec,
			currentSec: secondsFromClientX(event.clientX),
			altKey: event.altKey
		});
	}

	function onPointerUp() {
		drag = null;
		window.removeEventListener('pointermove', onPointerMove);
		window.removeEventListener('pointerup', onPointerUp);
		dispatch('dragend');
	}
</script>

<div class="loop" style="left: {startPx}px; width: {Math.max(0, endPx - startPx)}px">
	<div class="span"></div>
	<div class="edge edge--start" on:pointerdown={(e) => onPointerDown(e, 'start')}></div>
	<div class="edge edge--end" on:pointerdown={(e) => onPointerDown(e, 'end')}></div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.loop {
		position: absolute;
		top: 0;
		height: 100%;
		z-index: 4;
		pointer-events: none;
	}

	.span {
		position: absolute;
		inset: 0;
		background: $text-faint;
		pointer-events: none;
	}

	.edge {
		position: absolute;
		top: 0;
		width: 10px;
		height: 100%;
		cursor: ew-resize;
		pointer-events: auto;

		&::before,
		&::after {
			content: '';
			position: absolute;
			pointer-events: none;
		}

		&::after {
			top: 0;
			bottom: 0;
			width: 2px;
			background: $text-muted;
		}

		&--start {
			left: -4px;

			&::before {
				top: 0;
				left: 4px;
				border-top: 7px solid $text-muted;
				border-right: 6px solid transparent;
			}

			&::after {
				left: 4px;
			}
		}

		&--end {
			right: -4px;

			&::before {
				top: 0;
				right: 4px;
				border-top: 7px solid $text-muted;
				border-left: 6px solid transparent;
			}

			&::after {
				right: 4px;
			}
		}
	}
</style>
