<!-- ZoomScrollbar.svelte — scrollbar avec poignées de zoom/défilement -->
<script lang="ts">
	import { createEventDispatcher } from 'svelte';

	/** Position de début du pouce (fraction 0–1). */
	export let start = 0;
	/** Position de fin du pouce (fraction 0–1). */
	export let end = 1;
	/** Orientation du scrollbar. */
	export let orientation: 'h' | 'v' = 'h';
	/** Taille minimale du pouce en fraction. */
	export let minSize = 0.03;
	/** Id de la région défilée (`aria-controls`). */
	export let controls: string;

	const dispatch = createEventDispatcher<{
		change: { start: number; end: number; cause: 'pan' | 'resize-start' | 'resize-end' };
	}>();

	const GRIP_PX = 7;

	let track: HTMLDivElement;

	type DragKind = 'pan' | 'resize-start' | 'resize-end';
	let dragging: DragKind | null = null;
	let dragOriginPx = 0;
	let snapStart = 0;
	let snapEnd = 0;
	let cursor = 'default';

	$: isH = orientation === 'h';

	function trackSize(): number {
		if (!track) return 100;
		return isH ? track.clientWidth : track.clientHeight;
	}

	function clientPos(e: MouseEvent): number {
		return isH ? e.clientX : e.clientY;
	}

	function clamp(v: number, lo: number, hi: number): number {
		return Math.max(lo, Math.min(hi, v));
	}

	function getLocalFrac(e: MouseEvent): number {
		if (!track) return 0;
		const sz = trackSize();
		const rect = track.getBoundingClientRect();
		const base = isH ? rect.left : rect.top;
		return (clientPos(e) - base) / sz;
	}

	function classifyZone(localFrac: number): DragKind | 'outside' {
		const sz = trackSize();
		const gf = GRIP_PX / sz;
		if (localFrac < start - gf * 0.5 || localFrac > end + gf * 0.5) return 'outside';
		if (localFrac <= start + gf) return 'resize-start';
		if (localFrac >= end - gf) return 'resize-end';
		return 'pan';
	}

	function resizeCursor(): string {
		return isH ? 'ew-resize' : 'ns-resize';
	}

	function onTrackMouseMove(e: MouseEvent) {
		if (dragging) return;
		const zone = classifyZone(getLocalFrac(e));
		if (zone === 'resize-start' || zone === 'resize-end') cursor = resizeCursor();
		else if (zone === 'pan') cursor = 'grab';
		else cursor = 'default';
	}

	function onTrackMouseLeave() {
		if (!dragging) cursor = 'default';
	}

	function onTrackDown(e: MouseEvent) {
		if (e.button !== 0) return;
		e.preventDefault();
		e.stopPropagation();

		const lf = getLocalFrac(e);
		const zone = classifyZone(lf);

		if (zone === 'outside') {
			const thumbSz = end - start;
			const ns = clamp(lf - thumbSz / 2, 0, 1 - thumbSz);
			dispatch('change', { start: ns, end: ns + thumbSz, cause: 'pan' });
			return;
		}

		dragging = zone;
		dragOriginPx = clientPos(e);
		snapStart = start;
		snapEnd = end;
		cursor = zone === 'pan' ? 'grabbing' : resizeCursor();

		window.addEventListener('mousemove', onWindowMove);
		window.addEventListener('mouseup', onWindowUp, { once: true });
	}

	function onWindowMove(e: MouseEvent) {
		if (!dragging) return;
		const sz = trackSize();
		const delta = (clientPos(e) - dragOriginPx) / sz;
		let s = snapStart;
		let en = snapEnd;

		if (dragging === 'pan') {
			const thumbSz = en - s;
			s = clamp(snapStart + delta, 0, 1 - thumbSz);
			en = s + thumbSz;
		} else if (dragging === 'resize-start') {
			s = clamp(snapStart + delta, 0, snapEnd - minSize);
			en = snapEnd;
		} else {
			s = snapStart;
			en = clamp(snapEnd + delta, snapStart + minSize, 1);
		}

		dispatch('change', { start: s, end: en, cause: dragging });
	}

	function onWindowUp() {
		dragging = null;
		cursor = 'default';
		window.removeEventListener('mousemove', onWindowMove);
	}

	$: thumbStyle = isH
		? `left:${start * 100}%;width:${(end - start) * 100}%`
		: `top:${start * 100}%;height:${(end - start) * 100}%`;
</script>

<div
	class="zsb zsb--{orientation}"
	class:zsb--dragging={dragging !== null}
	bind:this={track}
	on:mousedown={onTrackDown}
	on:mousemove={onTrackMouseMove}
	on:mouseleave={onTrackMouseLeave}
	style:cursor
	role="scrollbar"
	tabindex="0"
	aria-controls={controls}
	aria-orientation={isH ? 'horizontal' : 'vertical'}
	aria-valuenow={Math.round(((start + end) / 2) * 100)}
	aria-valuemin={0}
	aria-valuemax={100}
>
	<div class="thumb" style={thumbStyle}>
		<div class="grip grip--start"></div>
		<div class="grip grip--end"></div>
	</div>
</div>

<style lang="scss">
	.zsb {
		position: relative;
		background: transparent;
		flex-shrink: 0;
		overflow: hidden;

		&--h {
			width: 100%;
			height: 9px;
		}
		&--v {
			height: 100%;
			width: 9px;
		}
	}

	.thumb {
		position: absolute;
		background: rgba(255, 255, 255, 0.09);
		border-radius: 4px;
		transition: background 0.12s;

		.zsb--h & {
			top: 0;
			bottom: 0;
			min-width: 20px;
		}
		.zsb--v & {
			left: 0;
			right: 0;
			min-height: 20px;
		}

		&:hover {
			background: rgba(255, 255, 255, 0.16);
		}
	}

	.grip {
		position: absolute;
		background: rgba(255, 255, 255, 0.4);
		border-radius: 1px;
		pointer-events: none;

		.zsb--h & {
			top: 50%;
			transform: translateY(-50%);
			width: 2px;
			height: 50%;

			&--start {
				left: 3px;
			}
			&--end {
				right: 3px;
			}
		}

		.zsb--v & {
			left: 50%;
			transform: translateX(-50%);
			height: 2px;
			width: 50%;

			&--start {
				top: 3px;
			}
			&--end {
				bottom: 3px;
			}
		}
	}
</style>
