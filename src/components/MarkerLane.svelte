<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import type { ArrangementMarker, ArrangementRegion } from '../types/global';
	import { MARKER_LANE_HEIGHT, MARKER_ROW_HEIGHT, REGION_ROW_HEIGHT } from '../frontend/timeRuler/constants';
	import { trackColor } from '../frontend/trackColor';
	import {
		addMarker,
		addRegion,
		removeMarker,
		removeRegion,
		updateMarker,
		updateRegion,
	} from '../frontend/commands/dispatcher';
	import LocatorContextMenu from './LocatorContextMenu.svelte';
	import { defaultRegionSpanAt } from '../frontend/arrangement/locators';

	export let markers: ArrangementMarker[] = [];
	export let regions: ArrangementRegion[] = [];
	export let pxPerBeat: number;
	export let scrollPx = 0;
	export let numerator = 4;
	/** Snap a beat position (Alt = free). */
	export let snapBeat: (beat: number, altKey: boolean) => number;
	/** Convert clientX → beat under the lane. */
	export let beatsFromClientX: (clientX: number) => number;
	/** Seek transport to a beat position. */
	export let seekToBeat: (beat: number) => void;
	/** Apply the ephemeral loop brace to a beat span. */
	export let setLoopFromBeats: (startBeat: number, endBeat: number) => void;

	const dispatch = createEventDispatcher<{
		focus: void;
	}>();

	let laneEl: HTMLDivElement | undefined;
	/** `marker:id` / `region:id` while inline-editing — keeps Svelte from rewriting the label. */
	let renamingKey: string | null = null;
	let renameSession: {
		el: HTMLElement;
		kind: 'marker' | 'region';
		id: string;
		original: string;
		onKeyDown: (e: KeyboardEvent) => void;
		onBlur: () => void;
	} | null = null;

	type DragState =
		| {
				kind: 'marker';
				id: string;
				originBeat: number;
				pointerOriginBeat: number;
				pointerOriginClientX: number;
		  }
		| {
				kind: 'region-move' | 'region-start' | 'region-end';
				id: string;
				originStart: number;
				originEnd: number;
				pointerOriginBeat: number;
				pointerOriginClientX: number;
		  };

	const DRAG_THRESHOLD_PX = 3;
	/** Delay before treating a click as seek, so dblclick can cancel it and rename. */
	const CLICK_DELAY_MS = 250;

	let drag: DragState | null = null;
	/** True once the pointer moved past the click→drag threshold. */
	let dragMoved = false;
	let pendingClick: ReturnType<typeof setTimeout> | null = null;
	/** Optimistic positions while dragging (null = use store values). */
	let draftMarkers: ArrangementMarker[] | null = null;
	let draftRegions: ArrangementRegion[] | null = null;

	$: displayMarkers = draftMarkers ?? markers;
	$: displayRegions = draftRegions ?? regions;

	type MenuState =
		| { mode: 'empty'; x: number; y: number; beat: number }
		| {
				mode: 'locator';
				x: number;
				y: number;
				kind: 'marker' | 'region';
				id: string;
				name: string;
		  };

	let menu: MenuState | null = null;

	function colorFor(id: string, override?: string | null): string {
		return trackColor(id, override);
	}

	function leftPx(beats: number): number {
		return beats * pxPerBeat - scrollPx;
	}

	function widthPx(start: number, end: number): number {
		return Math.max(2, (end - start) * pxPerBeat);
	}

	function focusLane() {
		dispatch('focus');
	}

	function onLaneMouseDown(e: MouseEvent) {
		if (e.button !== 0) return;
		if ((e.target as HTMLElement).closest('[data-locator]')) return;
		focusLane();
	}

	function onMarkersRowDblClick(e: MouseEvent) {
		if ((e.target as HTMLElement).closest('[data-locator]')) return;
		const beat = snapBeat(beatsFromClientX(e.clientX), e.altKey);
		addMarker(beat).catch(console.error);
	}

	function onRegionsRowDblClick(e: MouseEvent) {
		if ((e.target as HTMLElement).closest('[data-locator]')) return;
		const beat = beatsFromClientX(e.clientX);
		const { start, end } = defaultRegionSpanAt(beat, numerator);
		addRegion(start, end).catch(console.error);
	}

	function onLaneContextMenu(e: MouseEvent) {
		e.preventDefault();
		e.stopPropagation();
		const beat = snapBeat(beatsFromClientX(e.clientX), e.altKey);
		menu = { mode: 'empty', x: e.clientX, y: e.clientY, beat };
	}

	function cancelPendingClick() {
		if (pendingClick == null) return;
		clearTimeout(pendingClick);
		pendingClick = null;
	}

	function scheduleClick(action: () => void) {
		cancelPendingClick();
		pendingClick = setTimeout(() => {
			pendingClick = null;
			action();
		}, CLICK_DELAY_MS);
	}

	function onMarkerPointerDown(e: PointerEvent, marker: ArrangementMarker) {
		if (e.button !== 0) return;
		if ((e.target as HTMLElement).isContentEditable) return;
		e.stopPropagation();
		focusLane();
		dragMoved = false;
		drag = {
			kind: 'marker',
			id: marker.id,
			originBeat: marker.position_beats,
			pointerOriginBeat: beatsFromClientX(e.clientX),
			pointerOriginClientX: e.clientX,
		};
		draftMarkers = markers.map((m) => ({ ...m }));
		window.addEventListener('pointermove', onPointerMove);
		window.addEventListener('pointerup', onPointerUp);
	}

	function onMarkerDblClick(e: MouseEvent, marker: ArrangementMarker) {
		e.stopPropagation();
		e.preventDefault();
		cancelPendingClick();
		startRename('marker', marker.id, marker.name);
	}

	function goToRegion(startBeats: number, endBeats: number) {
		seekToBeat(startBeats);
		setLoopFromBeats(startBeats, endBeats);
	}

	function onRegionPointerDown(
		e: PointerEvent,
		region: ArrangementRegion,
		edge: 'move' | 'start' | 'end',
	) {
		if (e.button !== 0) return;
		if ((e.target as HTMLElement).isContentEditable) return;
		e.stopPropagation();
		focusLane();
		dragMoved = false;
		drag = {
			kind: edge === 'move' ? 'region-move' : edge === 'start' ? 'region-start' : 'region-end',
			id: region.id,
			originStart: region.start_beats,
			originEnd: region.end_beats,
			pointerOriginBeat: beatsFromClientX(e.clientX),
			pointerOriginClientX: e.clientX,
		};
		draftRegions = regions.map((r) => ({ ...r }));
		window.addEventListener('pointermove', onPointerMove);
		window.addEventListener('pointerup', onPointerUp);
	}

	function onRegionDblClick(e: MouseEvent, region: ArrangementRegion) {
		e.stopPropagation();
		e.preventDefault();
		cancelPendingClick();
		startRename('region', region.id, region.name);
	}

	function onPointerMove(e: PointerEvent) {
		if (!drag) return;
		if (!dragMoved) {
			if (Math.abs(e.clientX - drag.pointerOriginClientX) < DRAG_THRESHOLD_PX) return;
			dragMoved = true;
			cancelPendingClick();
		}
		const current = beatsFromClientX(e.clientX);
		const delta = current - drag.pointerOriginBeat;
		if (drag.kind === 'marker') {
			const next = snapBeat(Math.max(0, drag.originBeat + delta), e.altKey);
			draftMarkers = (draftMarkers ?? markers).map((m) =>
				m.id === drag!.id ? { ...m, position_beats: next } : m,
			);
			return;
		}
		const span = drag.originEnd - drag.originStart;
		let start = drag.originStart;
		let end = drag.originEnd;
		if (drag.kind === 'region-move') {
			start = snapBeat(Math.max(0, drag.originStart + delta), e.altKey);
			end = start + span;
		} else if (drag.kind === 'region-start') {
			start = snapBeat(Math.max(0, drag.originStart + delta), e.altKey);
			end = Math.max(start + 1, drag.originEnd);
		} else {
			end = snapBeat(Math.max(drag.originStart + 1, drag.originEnd + delta), e.altKey);
		}
		draftRegions = (draftRegions ?? regions).map((r) =>
			r.id === drag!.id ? { ...r, start_beats: start, end_beats: end } : r,
		);
	}

	function onPointerUp() {
		const finished = drag;
		const moved = dragMoved;
		drag = null;
		dragMoved = false;
		window.removeEventListener('pointermove', onPointerMove);
		window.removeEventListener('pointerup', onPointerUp);

		if (!finished) {
			draftMarkers = null;
			draftRegions = null;
			return;
		}

		if (!moved) {
			draftMarkers = null;
			draftRegions = null;
			if (finished.kind === 'marker') {
				scheduleClick(() => seekToBeat(finished.originBeat));
			} else if (finished.kind === 'region-move') {
				scheduleClick(() => goToRegion(finished.originStart, finished.originEnd));
			}
			return;
		}

		if (finished.kind === 'marker') {
			const draft = (draftMarkers ?? markers).find((m) => m.id === finished.id);
			draftMarkers = null;
			if (draft && Math.abs(draft.position_beats - finished.originBeat) > 1e-9) {
				updateMarker(finished.id, { position_beats: draft.position_beats }).catch(console.error);
			}
			return;
		}

		const draft = (draftRegions ?? regions).find((r) => r.id === finished.id);
		draftRegions = null;
		if (
			draft &&
			(Math.abs(draft.start_beats - finished.originStart) > 1e-9 ||
				Math.abs(draft.end_beats - finished.originEnd) > 1e-9)
		) {
			updateRegion(finished.id, {
				start_beats: draft.start_beats,
				end_beats: draft.end_beats,
			}).catch(console.error);
		}
	}

	function startRename(kind: 'marker' | 'region', id: string, name: string) {
		cancelPendingClick();
		if (drag) {
			drag = null;
			dragMoved = false;
			draftMarkers = null;
			draftRegions = null;
			window.removeEventListener('pointermove', onPointerMove);
			window.removeEventListener('pointerup', onPointerUp);
		}
		if (renameSession) {
			renameSession.el.removeEventListener('keydown', renameSession.onKeyDown);
			renameSession.el.removeEventListener('blur', renameSession.onBlur);
			finishInlineRename(renameSession.el, renameSession.kind, renameSession.id, renameSession.original, false);
			renameSession = null;
		}

		renamingKey = `${kind}:${id}`;

		requestAnimationFrame(() => {
			const el = laneEl?.querySelector(`[data-label="${kind}:${id}"]`);
			if (!(el instanceof HTMLElement)) {
				renamingKey = null;
				return;
			}

			const original = name;
			el.textContent = original;
			el.contentEditable = 'true';
			el.focus();

			const range = document.createRange();
			range.selectNodeContents(el);
			const sel = window.getSelection();
			sel?.removeAllRanges();
			sel?.addRange(range);

			const onKeyDown = (event: KeyboardEvent) => {
				if (event.key === 'Enter') {
					event.preventDefault();
					el.blur();
				} else if (event.key === 'Escape') {
					event.preventDefault();
					el.textContent = original;
					el.blur();
				}
			};

			const onBlur = () => {
				el.removeEventListener('keydown', onKeyDown);
				el.removeEventListener('blur', onBlur);
				renameSession = null;
				finishInlineRename(el, kind, id, original, true);
			};

			el.addEventListener('keydown', onKeyDown);
			el.addEventListener('blur', onBlur);
			renameSession = { el, kind, id, original, onKeyDown, onBlur };
		});
	}

	function finishInlineRename(
		el: HTMLElement,
		kind: 'marker' | 'region',
		id: string,
		original: string,
		commit: boolean,
	) {
		el.contentEditable = 'false';
		const trimmed = (el.textContent ?? '').trim();
		renamingKey = null;
		if (!commit || !trimmed || trimmed === original) return;
		if (kind === 'marker') {
			updateMarker(id, { name: trimmed }).catch(console.error);
		} else {
			updateRegion(id, { name: trimmed }).catch(console.error);
		}
	}

	function onMarkerContextMenu(e: MouseEvent, marker: ArrangementMarker) {
		e.preventDefault();
		e.stopPropagation();
		focusLane();
		menu = {
			mode: 'locator',
			x: e.clientX,
			y: e.clientY,
			kind: 'marker',
			id: marker.id,
			name: marker.name,
		};
	}

	function onRegionContextMenu(e: MouseEvent, region: ArrangementRegion) {
		e.preventDefault();
		e.stopPropagation();
		focusLane();
		menu = {
			mode: 'locator',
			x: e.clientX,
			y: e.clientY,
			kind: 'region',
			id: region.id,
			name: region.name,
		};
	}
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	bind:this={laneEl}
	class="marker-lane"
	class:marker-lane--empty={displayMarkers.length === 0 && displayRegions.length === 0}
	style="height: {MARKER_LANE_HEIGHT}px; --marker-row: {MARKER_ROW_HEIGHT}px; --region-row: {REGION_ROW_HEIGHT}px"
	on:mousedown={onLaneMouseDown}
	on:contextmenu={onLaneContextMenu}
>
	{#if displayMarkers.length === 0 && displayRegions.length === 0}
		<span class="empty-hint">
			<span class="empty-hint__key">M</span> marqueur
			<span class="empty-hint__sep">·</span>
			<span class="empty-hint__key">R</span> région
			<span class="empty-hint__sep">·</span>
			clic droit
		</span>
	{/if}

	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div
		class="marker-lane__markers"
		style="height: {MARKER_ROW_HEIGHT}px"
		on:dblclick={onMarkersRowDblClick}
	>
		{#each displayMarkers as marker (marker.id)}
			{@const color = colorFor(marker.id, marker.color)}
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<div
				class="marker"
				class:dragging={drag?.kind === 'marker' && drag.id === marker.id}
				data-locator="marker"
				style="left: {leftPx(marker.position_beats)}px; --loc-color: {color}"
				on:pointerdown={(e) => onMarkerPointerDown(e, marker)}
				on:dblclick={(e) => onMarkerDblClick(e, marker)}
				on:contextmenu={(e) => onMarkerContextMenu(e, marker)}
				title={marker.name}
			>
				<div class="marker__flag"></div>
				{#if renamingKey === `marker:${marker.id}`}
					<span class="marker-label is-editing" data-label="marker:{marker.id}"></span>
				{:else}
					<span class="marker-label" data-label="marker:{marker.id}">{marker.name}</span>
				{/if}
			</div>
		{/each}
	</div>

	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div
		class="marker-lane__regions"
		style="height: {REGION_ROW_HEIGHT}px"
		on:dblclick={onRegionsRowDblClick}
	>
		{#each displayRegions as region (region.id)}
			{@const color = colorFor(region.id, region.color)}
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<div
				class="region"
				class:dragging={drag != null && drag.id === region.id}
				data-locator="region"
				style="left: {leftPx(region.start_beats)}px; width: {widthPx(region.start_beats, region.end_beats)}px; --loc-color: {color}"
				on:pointerdown={(e) => onRegionPointerDown(e, region, 'move')}
				on:dblclick={(e) => onRegionDblClick(e, region)}
				on:contextmenu={(e) => onRegionContextMenu(e, region)}
				title={region.name}
			>
				<div
					class="region-edge region-edge--start"
					on:pointerdown={(e) => onRegionPointerDown(e, region, 'start')}
				></div>
				{#if renamingKey === `region:${region.id}`}
					<span class="region-label is-editing" data-label="region:{region.id}"></span>
				{:else}
					<span class="region-label" data-label="region:{region.id}">{region.name}</span>
				{/if}
				<div
					class="region-edge region-edge--end"
					on:pointerdown={(e) => onRegionPointerDown(e, region, 'end')}
				></div>
			</div>
		{/each}
	</div>
</div>

{#if menu}
	<LocatorContextMenu
		x={menu.x}
		y={menu.y}
		mode={menu.mode}
		kind={menu.mode === 'locator' ? menu.kind : null}
		name={menu.mode === 'locator' ? menu.name : ''}
		on:addMarker={() => {
			const current = menu;
			if (current?.mode === 'empty') addMarker(current.beat).catch(console.error);
		}}
		on:addRegion={() => {
			const current = menu;
			if (current?.mode === 'empty') {
				const { start, end } = defaultRegionSpanAt(current.beat, numerator);
				addRegion(start, end).catch(console.error);
			}
		}}
		on:rename={() => {
			const current = menu;
			if (current?.mode === 'locator') startRename(current.kind, current.id, current.name);
		}}
		on:goTo={() => {
			const current = menu;
			if (current?.mode !== 'locator') return;
			if (current.kind === 'marker') {
				const marker = markers.find((m) => m.id === current.id);
				if (marker) seekToBeat(marker.position_beats);
			} else {
				const region = regions.find((r) => r.id === current.id);
				if (region) goToRegion(region);
			}
		}}
		on:setLoop={() => {
			const current = menu;
			if (current?.mode === 'locator' && current.kind === 'region') {
				const region = regions.find((r) => r.id === current.id);
				if (region) setLoopFromBeats(region.start_beats, region.end_beats);
			}
		}}
		on:delete={() => {
			const current = menu;
			if (current?.mode !== 'locator') return;
			if (current.kind === 'marker') removeMarker(current.id).catch(console.error);
			else removeRegion(current.id).catch(console.error);
		}}
		on:close={() => {
			menu = null;
		}}
	/>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.marker-lane {
		position: relative;
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		background: linear-gradient(180deg, rgba(255, 255, 255, 0.025) 0%, $background-color 100%);
		border-bottom: 1px solid $border-color;
		cursor: default;
		user-select: none;
		width: 100%;

		&--empty {
			cursor: cell;
		}
	}

	.marker-lane__markers,
	.marker-lane__regions {
		position: relative;
		box-sizing: border-box;
		flex-shrink: 0;
		width: 100%;
		overflow: hidden;
	}

	.marker-lane__markers {
		border-bottom: 1px solid $border-faint;
	}

	.empty-hint {
		position: absolute;
		inset: 0;
		z-index: 2;
		display: flex;
		align-items: center;
		padding-left: 10px;
		gap: 5px;
		font-size: 10px;
		line-height: 1;
		color: $text-faint;
		pointer-events: none;
		white-space: nowrap;
		letter-spacing: 0.01em;
	}

	.empty-hint__key {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-width: 14px;
		height: 14px;
		padding: 0 3px;
		border-radius: 3px;
		background: $surface-2;
		border: 1px solid $border-faint;
		color: $text-subtle;
		font-family: $font-mono;
		font-size: 9px;
	}

	.empty-hint__sep {
		opacity: 0.5;
		margin: 0 1px;
	}

	.region {
		box-sizing: border-box;
		position: absolute;
		top: 1px;
		bottom: 1px;
		display: flex;
		align-items: center;
		padding: 0 7px;
		border-radius: 3px;
		background: color-mix(in srgb, var(--loc-color) 18%, transparent);
		border: 1px solid color-mix(in srgb, var(--loc-color) 42%, transparent);
		cursor: pointer;
		z-index: 1;
		min-width: 10px;
		overflow: hidden;
		transition: background 0.12s ease, border-color 0.12s ease, box-shadow 0.12s ease;

		&:hover {
			background: color-mix(in srgb, var(--loc-color) 28%, transparent);
			border-color: color-mix(in srgb, var(--loc-color) 60%, transparent);
		}

		&.dragging {
			cursor: grabbing;
		}
	}

	.region-label,
	.marker-label {
		font-size: 10px;
		font-weight: 500;
		line-height: 1.2;
		color: $text-color;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		pointer-events: auto;
		cursor: inherit;
		user-select: none;
		text-shadow: 0 1px 2px rgba(0, 0, 0, 0.45);
		border: none;
		outline: none;
		box-shadow: none;
		caret-color: $accent-color;

		&.is-editing {
			overflow: visible;
			text-overflow: clip;
			cursor: text;
			user-select: text;
			-webkit-user-select: text;

			&::selection {
				background: rgba($accent-color, 0.45);
				color: $text-color;
			}

			&::-moz-selection {
				background: rgba($accent-color, 0.45);
				color: $text-color;
			}
		}

		&:focus {
			outline: none;
		}
	}

	.marker-label {
		padding: 0 3px;
		border-radius: 2px;
		background: rgba(26, 26, 31, 0.82);
		max-width: 72px;
	}

	.region-edge {
		position: absolute;
		top: 0;
		bottom: 0;
		width: 7px;
		cursor: ew-resize;
		z-index: 2;

		&--start {
			left: 0;
		}

		&--end {
			right: 0;
		}

		&:hover::after {
			content: '';
			position: absolute;
			top: 2px;
			bottom: 2px;
			width: 2px;
			background: color-mix(in srgb, var(--loc-color) 80%, white);
			border-radius: 1px;
		}

		&--start:hover::after {
			left: 1px;
		}

		&--end:hover::after {
			right: 1px;
		}
	}

	.marker {
		position: absolute;
		top: 0;
		height: 100%;
		display: flex;
		align-items: center;
		gap: 2px;
		transform: translateX(-3px);
		cursor: pointer;
		z-index: 3;
		padding-right: 2px;

		&.dragging {
			cursor: grabbing;
		}

		&:hover .marker__flag {
			filter: brightness(1.12);
		}
	}

	.marker__flag {
		position: relative;
		z-index: 1;
		width: 7px;
		height: 9px;
		flex-shrink: 0;
		background: var(--loc-color);
		clip-path: polygon(0 0, 100% 0, 100% 70%, 50% 100%, 0 70%);
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.35);
		cursor: inherit;
	}
</style>
