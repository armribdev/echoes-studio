<!-- GraphView.svelte — read-only audio graph visualization -->
<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { get } from 'svelte/store';
	import { graphSnapshot } from '../stores/graphSnapshot';
	import { viewMode, type GraphScope } from '../stores/viewMode';
	import { selectedTrack } from '../stores/selectedTrack';
	import { projectEventStore } from '../shared/project/eventStore';
	import { leftPanel } from '../stores/leftPanel';
	import {
		layoutTrackGraph,
		layoutProjectGraph,
		visibleAudioEdges,
		visibleModEdges,
	} from '../frontend/graph/layout';
	import { renderGraph, hitTestNode } from '../frontend/graph/render';
	import type { GraphLayout, LayoutNode } from '../frontend/graph/types';

	const MIN_ZOOM = 0.25;
	const MAX_ZOOM = 2;
	const dpr = typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1;

	let canvas: HTMLCanvasElement;
	let container: HTMLDivElement;
	let panX = 0;
	let panY = 0;
	let zoom = 1;
	let hoveredNodeId: number | null = null;
	let modPhase = 0;
	let animFrame = 0;
	let isPanning = false;
	let panStartX = 0;
	let panStartY = 0;
	let panOriginX = 0;
	let panOriginY = 0;

	$: tracks = $projectEventStore.tracks;
	$: trackId = $selectedTrack.trackId;
	$: scope = $viewMode.graphScope;
	$: snapshot = $graphSnapshot;

	$: trackColor = (() => {
		if (!trackId) return '#4d96ff';
		const idx = tracks.findIndex((t) => t.id === trackId);
		const colors = [
			'#4d96ff', '#ff6b6b', '#6bcb77', '#ffd93d',
			'#c77dff', '#ff9a3c', '#00c9a7', '#ff6db6',
		];
		return colors[Math.max(0, idx) % colors.length];
	})();

	$: layout = computeLayout(snapshot, scope, trackId);
	$: audioEdges = snapshot ? visibleAudioEdges(snapshot, layout) : [];
	$: modEdges = snapshot ? visibleModEdges(snapshot, layout) : [];
	$: selectedNodeId = (() => {
		if (!trackId || !snapshot || scope !== 'track') return null;
		const gain = snapshot.nodes.find(
			(n) => n.trackId === trackId && n.kind === 'track_gain',
		);
		return gain?.id ?? null;
	})();

	function computeLayout(
		snap: typeof snapshot,
		sc: GraphScope,
		tid: string | null,
	): GraphLayout {
		if (!snap) return { nodes: [], width: 400, height: 200 };
		if (sc === 'project') return layoutProjectGraph(snap);
		if (!tid) return { nodes: [], width: 400, height: 200 };
		return layoutTrackGraph(snap, tid);
	}

	function fitToView() {
		if (!container || !layout) return;
		const cw = container.clientWidth;
		const ch = container.clientHeight;
		if (layout.width <= 0 || layout.height <= 0) return;
		const pad = 32;
		zoom = Math.min(
			MAX_ZOOM,
			Math.max(MIN_ZOOM, Math.min((cw - pad) / layout.width, (ch - pad) / layout.height)),
		);
		panX = (cw - layout.width * zoom) / 2;
		panY = (ch - layout.height * zoom) / 2;
	}

	function paint() {
		if (!canvas || !container) return;
		const cw = container.clientWidth;
		const ch = container.clientHeight;
		if (cw <= 0 || ch <= 0) return;

		canvas.width = cw * dpr;
		canvas.height = ch * dpr;
		canvas.style.width = `${cw}px`;
		canvas.style.height = `${ch}px`;

		const ctx = canvas.getContext('2d');
		if (!ctx) return;

		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, cw, ch);
		ctx.save();
		ctx.translate(panX, panY);
		ctx.scale(zoom, zoom);

		renderGraph(
			ctx,
			layout,
			audioEdges,
			modEdges,
			{
				trackColor,
				hoveredNodeId,
				selectedNodeId,
				modPhase,
				dpr,
			},
			layout.width,
			layout.height,
		);

		ctx.restore();
	}

	function onPointerDown(e: PointerEvent) {
		if (e.button !== 0 && e.button !== 1) return;
		const rect = canvas.getBoundingClientRect();
		const wx = (e.clientX - rect.left - panX) / zoom;
		const wy = (e.clientY - rect.top - panY) / zoom;
		const hit = hitTestNode(layout, wx, wy);

		if (hit) {
			selectNodeTrack(hit);
			return;
		}

		isPanning = true;
		panStartX = e.clientX;
		panStartY = e.clientY;
		panOriginX = panX;
		panOriginY = panY;
		canvas.setPointerCapture(e.pointerId);
	}

	function onPointerMove(e: PointerEvent) {
		const rect = canvas.getBoundingClientRect();
		const wx = (e.clientX - rect.left - panX) / zoom;
		const wy = (e.clientY - rect.top - panY) / zoom;

		if (isPanning) {
			panX = panOriginX + (e.clientX - panStartX);
			panY = panOriginY + (e.clientY - panStartY);
			paint();
			return;
		}

		const hit = hitTestNode(layout, wx, wy);
		const next = hit?.id ?? null;
		if (next !== hoveredNodeId) {
			hoveredNodeId = next;
			paint();
		}
	}

	function onPointerUp(e: PointerEvent) {
		isPanning = false;
		canvas.releasePointerCapture(e.pointerId);
	}

	function onWheel(e: WheelEvent) {
		e.preventDefault();
		const rect = canvas.getBoundingClientRect();
		const mx = e.clientX - rect.left;
		const my = e.clientY - rect.top;
		const factor = e.deltaY < 0 ? 1.08 : 1 / 1.08;
		const newZoom = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom * factor));
		panX = mx - ((mx - panX) / zoom) * newZoom;
		panY = my - ((my - panY) / zoom) * newZoom;
		zoom = newZoom;
		paint();
	}

	function selectNodeTrack(node: LayoutNode) {
		const tid = node.dto.trackId;
		if (!tid) return;

		const idx = tracks.findIndex((t) => t.id === tid);
		if (idx < 0) return;

		const colors = [
			'#4d96ff', '#ff6b6b', '#6bcb77', '#ffd93d',
			'#c77dff', '#ff9a3c', '#00c9a7', '#ff6db6',
		];
		const t = tracks[idx];
		selectedTrack.set({
			trackId: t.id,
			trackName: t.name,
			trackColor: colors[idx % colors.length],
			instrument: t.instrument ?? null,
			effects: t.effects ?? [],
			modulators: t.modulators ?? [],
			mod_routes: t.mod_routes ?? [],
		});

		if (!get(leftPanel).open) leftPanel.toggle();

		if (scope === 'project') {
			viewMode.setGraphScope('track');
		}
	}

	function focusProjectView() {
		viewMode.setGraphScope('project');
	}

	function focusTrackView() {
		viewMode.setGraphScope('track');
	}

	let resizeObs: ResizeObserver | undefined;

	onMount(() => {
		resizeObs = new ResizeObserver(() => {
			paint();
		});
		resizeObs.observe(container);

		const tickAnim = () => {
			modPhase += 0.04;
			if ($viewMode.main === 'graph') paint();
			animFrame = requestAnimationFrame(tickAnim);
		};
		animFrame = requestAnimationFrame(tickAnim);
	});

	$: if (layout && container) {
		tick().then(() => {
			fitToView();
			paint();
		});
	}

	$: if ($viewMode.main === 'graph' && snapshot) {
		tick().then(() => paint());
	}
</script>

<div class="graph-view" bind:this={container}>
	<div class="toolbar">
		<div class="scope-tabs">
			<button
				type="button"
				class:active={scope === 'track'}
				on:click={focusTrackView}
				disabled={!trackId}
				title="Chaîne de la piste sélectionnée"
			>
				Piste
			</button>
			<button
				type="button"
				class:active={scope === 'project'}
				on:click={focusProjectView}
				title="Vue projet entière"
			>
				Projet
			</button>
		</div>
		{#if scope === 'track' && trackId}
			<span class="track-label">{$selectedTrack.trackName || 'Piste'}</span>
		{/if}
		<button type="button" class="fit-btn" on:click={fitToView} title="Ajuster à la vue">
			Ajuster
		</button>
	</div>

	<canvas
		bind:this={canvas}
		class="graph-canvas"
		on:pointerdown={onPointerDown}
		on:pointermove={onPointerMove}
		on:pointerup={onPointerUp}
		on:pointerleave={onPointerUp}
		on:wheel={onWheel}
	></canvas>

	{#if !snapshot}
		<div class="empty">Chargement du graphe…</div>
	{:else if scope === 'track' && !trackId}
		<div class="empty">Sélectionnez une piste pour afficher sa chaîne de signal.</div>
	{:else if layout.nodes.length === 0}
		<div class="empty">Aucun nœud à afficher.</div>
	{/if}

	{#if hoveredNodeId !== null && snapshot}
		{@const node = layout.nodes.find((n) => n.id === hoveredNodeId)}
		{#if node}
			<div class="tooltip">{node.dto.label}{node.dto.summary ? ` · ${node.dto.summary}` : ''}</div>
		{/if}
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.graph-view {
		flex: 1;
		min-width: 0;
		min-height: 0;
		position: relative;
		display: flex;
		flex-direction: column;
		background: #1a1a1f;
		overflow: hidden;
	}

	.toolbar {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 6px 10px;
		border-bottom: 1px solid $border-color;
		background: #222229;
		flex-shrink: 0;
		z-index: 2;
	}

	.scope-tabs {
		display: flex;
		gap: 2px;
		background: rgba(0, 0, 0, 0.25);
		border-radius: $radius-md;
		padding: 2px;

		button {
			padding: 4px 12px;
			border: none;
			border-radius: 4px;
			background: transparent;
			color: $text-muted;
			font-size: 11px;
			font-family: inherit;
			cursor: pointer;

			&.active {
				background: rgba(21, 113, 255, 0.25);
				color: $text-color;
			}

			&:disabled {
				opacity: 0.4;
				cursor: default;
			}

			&:not(:disabled):hover {
				color: $text-color;
			}
		}
	}

	.track-label {
		font-size: 11px;
		color: $text-subtle;
		font-family: $font-mono;
	}

	.fit-btn {
		margin-left: auto;
		padding: 4px 10px;
		border: 1px solid $border-subtle;
		border-radius: 4px;
		background: transparent;
		color: $text-muted;
		font-size: 10px;
		cursor: pointer;

		&:hover {
			background: $surface-hover;
			color: $text-color;
		}
	}

	.graph-canvas {
		flex: 1;
		width: 100%;
		height: 100%;
		display: block;
		cursor: grab;

		&:active {
			cursor: grabbing;
		}
	}

	.empty {
		position: absolute;
		inset: 0;
		top: 40px;
		display: flex;
		align-items: center;
		justify-content: center;
		color: $text-faint;
		font-size: 12px;
		pointer-events: none;
	}

	.tooltip {
		position: absolute;
		bottom: 12px;
		left: 50%;
		transform: translateX(-50%);
		padding: 6px 12px;
		background: rgba(0, 0, 0, 0.75);
		border: 1px solid $border-subtle;
		border-radius: $radius-md;
		font-size: 10px;
		font-family: $font-mono;
		color: $text-color;
		pointer-events: none;
		max-width: 80%;
		text-align: center;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
</style>
