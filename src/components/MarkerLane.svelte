<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { get } from 'svelte/store';
	import type { ArrangementMarker, ArrangementRegion } from '../types/global';
	import { MARKER_LANE_HEIGHT } from '../frontend/timeRuler/constants';
	import { trackColor } from '../frontend/trackColor';
	import { locatorSelection } from '../stores/locatorSelection';
	import {
		addMarker,
		addRegion,
		removeMarker,
		removeRegion,
		updateMarker,
		updateRegion,
	} from '../frontend/commands/dispatcher';
	import LocatorContextMenu from './LocatorContextMenu.svelte';

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

	$: selected = $locatorSelection;

	let renamingId: string | null = null;
	let renameKind: 'marker' | 'region' | null = null;
	let renameValue = '';
	let renameInput: HTMLInputElement | null = null;

	type DragState =
		| {
				kind: 'marker';
				id: string;
				originBeat: number;
				pointerOriginBeat: number;
		  }
		| {
				kind: 'region-move' | 'region-start' | 'region-end';
				id: string;
				originStart: number;
				originEnd: number;
				pointerOriginBeat: number;
		  };

	let drag: DragState | null = null;
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

	function selectLocator(kind: 'marker' | 'region', id: string) {
		locatorSelection.select(kind, id);
		dispatch('focus');
	}

	function onLaneMouseDown(e: MouseEvent) {
		if (e.button !== 0) return;
		if ((e.target as HTMLElement).closest('[data-locator]')) return;
		locatorSelection.clear();
		dispatch('focus');
	}

	function onLaneDblClick(e: MouseEvent) {
		if ((e.target as HTMLElement).closest('[data-locator]')) return;
		const beat = snapBeat(beatsFromClientX(e.clientX), e.altKey);
		addMarker(beat).catch(console.error);
	}

	function onLaneContextMenu(e: MouseEvent) {
		e.preventDefault();
		e.stopPropagation();
		const beat = snapBeat(beatsFromClientX(e.clientX), e.altKey);
		menu = { mode: 'empty', x: e.clientX, y: e.clientY, beat };
	}

	function onMarkerPointerDown(e: PointerEvent, marker: ArrangementMarker) {
		if (e.button !== 0) return;
		e.stopPropagation();
		e.preventDefault();
		selectLocator('marker', marker.id);
		seekToBeat(marker.position_beats);
		drag = {
			kind: 'marker',
			id: marker.id,
			originBeat: marker.position_beats,
			pointerOriginBeat: beatsFromClientX(e.clientX),
		};
		draftMarkers = markers.map((m) => ({ ...m }));
		window.addEventListener('pointermove', onPointerMove);
		window.addEventListener('pointerup', onPointerUp);
	}

	function onRegionPointerDown(
		e: PointerEvent,
		region: ArrangementRegion,
		edge: 'move' | 'start' | 'end',
	) {
		if (e.button !== 0) return;
		e.stopPropagation();
		e.preventDefault();
		selectLocator('region', region.id);
		seekToBeat(region.start_beats);
		drag = {
			kind: edge === 'move' ? 'region-move' : edge === 'start' ? 'region-start' : 'region-end',
			id: region.id,
			originStart: region.start_beats,
			originEnd: region.end_beats,
			pointerOriginBeat: beatsFromClientX(e.clientX),
		};
		draftRegions = regions.map((r) => ({ ...r }));
		window.addEventListener('pointermove', onPointerMove);
		window.addEventListener('pointerup', onPointerUp);
	}

	function onPointerMove(e: PointerEvent) {
		if (!drag) return;
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
		drag = null;
		window.removeEventListener('pointermove', onPointerMove);
		window.removeEventListener('pointerup', onPointerUp);

		if (!finished) {
			draftMarkers = null;
			draftRegions = null;
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
		renamingId = id;
		renameKind = kind;
		renameValue = name;
		requestAnimationFrame(() => {
			renameInput?.focus();
			renameInput?.select();
		});
	}

	function commitRename() {
		if (!renamingId || !renameKind) return;
		const trimmed = renameValue.trim();
		const id = renamingId;
		const kind = renameKind;
		renamingId = null;
		renameKind = null;
		if (!trimmed) return;
		if (kind === 'marker') {
			const current = markers.find((m) => m.id === id);
			if (current && current.name !== trimmed) {
				updateMarker(id, { name: trimmed }).catch(console.error);
			}
		} else {
			const current = regions.find((r) => r.id === id);
			if (current && current.name !== trimmed) {
				updateRegion(id, { name: trimmed }).catch(console.error);
			}
		}
	}

	function cancelRename() {
		renamingId = null;
		renameKind = null;
	}

	function onMarkerContextMenu(e: MouseEvent, marker: ArrangementMarker) {
		e.preventDefault();
		e.stopPropagation();
		selectLocator('marker', marker.id);
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
		selectLocator('region', region.id);
		menu = {
			mode: 'locator',
			x: e.clientX,
			y: e.clientY,
			kind: 'region',
			id: region.id,
			name: region.name,
		};
	}

	function defaultRegionSpanAt(beat: number): { start: number; end: number } {
		const bars = 4;
		const span = bars * numerator;
		const start = snapBeat(Math.max(0, beat), false);
		return { start, end: start + span };
	}

	export function deleteSelectedLocator() {
		const sel = get(locatorSelection);
		if (!sel) return false;
		if (sel.kind === 'marker') {
			removeMarker(sel.id).catch(console.error);
		} else {
			removeRegion(sel.id).catch(console.error);
		}
		locatorSelection.clear();
		return true;
	}
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	class="marker-lane"
	style="height: {MARKER_LANE_HEIGHT}px"
	on:mousedown={onLaneMouseDown}
	on:dblclick={onLaneDblClick}
	on:contextmenu={onLaneContextMenu}
>
	{#each displayRegions as region (region.id)}
		{@const color = colorFor(region.id, region.color)}
		{@const selectedHere = selected?.kind === 'region' && selected.id === region.id}
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<div
			class="region"
			class:selected={selectedHere}
			data-locator="region"
			style="left: {leftPx(region.start_beats)}px; width: {widthPx(region.start_beats, region.end_beats)}px; --loc-color: {color}"
			on:pointerdown={(e) => onRegionPointerDown(e, region, 'move')}
			on:dblclick|stopPropagation={() => startRename('region', region.id, region.name)}
			on:contextmenu={(e) => onRegionContextMenu(e, region)}
		>
			<div
				class="region-edge region-edge--start"
				on:pointerdown={(e) => onRegionPointerDown(e, region, 'start')}
			></div>
			{#if renamingId === region.id && renameKind === 'region'}
				<input
					bind:this={renameInput}
					class="rename"
					bind:value={renameValue}
					on:blur={commitRename}
					on:keydown={(e) => {
						if (e.key === 'Enter') {
							e.preventDefault();
							commitRename();
						} else if (e.key === 'Escape') {
							e.preventDefault();
							cancelRename();
						}
					}}
				/>
			{:else}
				<span class="region-label">{region.name}</span>
			{/if}
			<div
				class="region-edge region-edge--end"
				on:pointerdown={(e) => onRegionPointerDown(e, region, 'end')}
			></div>
		</div>
	{/each}

	{#each displayMarkers as marker (marker.id)}
		{@const color = colorFor(marker.id, marker.color)}
		{@const selectedHere = selected?.kind === 'marker' && selected.id === marker.id}
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<div
			class="marker"
			class:selected={selectedHere}
			data-locator="marker"
			style="left: {leftPx(marker.position_beats)}px; --loc-color: {color}"
			on:pointerdown={(e) => onMarkerPointerDown(e, marker)}
			on:dblclick|stopPropagation={() => startRename('marker', marker.id, marker.name)}
			on:contextmenu={(e) => onMarkerContextMenu(e, marker)}
		>
			<div class="marker-flag"></div>
			{#if renamingId === marker.id && renameKind === 'marker'}
				<input
					bind:this={renameInput}
					class="rename"
					bind:value={renameValue}
					on:blur={commitRename}
					on:keydown={(e) => {
						if (e.key === 'Enter') {
							e.preventDefault();
							commitRename();
						} else if (e.key === 'Escape') {
							e.preventDefault();
							cancelRename();
						}
					}}
				/>
			{:else}
				<span class="marker-label">{marker.name}</span>
			{/if}
		</div>
	{/each}
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
				const { start, end } = defaultRegionSpanAt(current.beat);
				addRegion(start, end).catch(console.error);
			}
		}}
		on:rename={() => {
			const current = menu;
			if (current?.mode === 'locator') startRename(current.kind, current.id, current.name);
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
			locatorSelection.clear();
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
		overflow: hidden;
		background: $background-color;
		border-bottom: 1px solid $border-color;
		cursor: default;
		user-select: none;
		width: 100%;
	}

	.region {
		position: absolute;
		top: 2px;
		bottom: 2px;
		display: flex;
		align-items: center;
		padding: 0 6px;
		border-radius: 3px;
		background: color-mix(in srgb, var(--loc-color) 28%, transparent);
		border: 1px solid color-mix(in srgb, var(--loc-color) 55%, transparent);
		cursor: grab;
		z-index: 1;
		min-width: 8px;

		&.selected {
			border-color: var(--loc-color);
			box-shadow: 0 0 0 1px color-mix(in srgb, var(--loc-color) 40%, transparent);
		}
	}

	.region-label,
	.marker-label {
		font-size: 10px;
		line-height: 1;
		color: $text-color;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		pointer-events: none;
	}

	.region-edge {
		position: absolute;
		top: 0;
		bottom: 0;
		width: 6px;
		cursor: ew-resize;
		z-index: 2;

		&--start {
			left: 0;
		}

		&--end {
			right: 0;
		}
	}

	.marker {
		position: absolute;
		top: 0;
		height: 100%;
		display: flex;
		align-items: flex-start;
		gap: 3px;
		transform: translateX(-4px);
		cursor: grab;
		z-index: 3;
		padding-right: 4px;

		&.selected .marker-flag {
			filter: brightness(1.15);
		}
	}

	.marker-flag {
		width: 0;
		height: 0;
		border-left: 4px solid transparent;
		border-right: 4px solid transparent;
		border-top: 10px solid var(--loc-color);
		flex-shrink: 0;
		margin-top: 2px;
	}

	.rename {
		width: 64px;
		height: 14px;
		padding: 0 3px;
		border: 1px solid $accent-color;
		border-radius: 2px;
		background: $panel-background-color;
		color: $text-color;
		font-size: 10px;
		font-family: inherit;
		outline: none;
	}
</style>
