import type {
	AudioEdgeDto,
	GraphLayout,
	GraphNodeDto,
	GraphSnapshotDto,
	LayoutNode,
	ModEdgeDto
} from './types';
import { H_GAP, LFO_OFFSET_Y, NODE_H, NODE_W, PADDING, TRACK_ROW_H } from './types';

function nodeMap(snapshot: GraphSnapshotDto): Map<number, GraphNodeDto> {
	return new Map(snapshot.nodes.map((n) => [n.id, n]));
}

function audioAdjacency(edges: AudioEdgeDto[]): Map<number, number[]> {
	const adj = new Map<number, number[]>();
	for (const e of edges) {
		const list = adj.get(e.fromNode) ?? [];
		list.push(e.toNode);
		adj.set(e.fromNode, list);
	}
	return adj;
}

/** Ordered audio chain from source → … → meter (excludes master). */
function chainFromSource(
	sourceId: number,
	adj: Map<number, number[]>,
	nodes: Map<number, GraphNodeDto>
): number[] {
	const chain: number[] = [sourceId];
	let current = sourceId;
	const stopKinds = new Set(['meter']);

	let keepGoing = true;
	while (keepGoing) {
		const nextIds = adj.get(current) ?? [];
		const next = nextIds.find((id) => {
			const k = nodes.get(id)?.kind;
			return k !== 'master';
		});
		if (next === undefined) {
			keepGoing = false;
			continue;
		}
		chain.push(next);
		if (stopKinds.has(nodes.get(next)?.kind ?? '')) {
			keepGoing = false;
			continue;
		}
		current = next;
	}
	return chain;
}

function placeRow(ids: number[], nodes: Map<number, GraphNodeDto>, y: number): LayoutNode[] {
	return ids.map((id, i) => ({
		id,
		x: PADDING + i * (NODE_W + H_GAP),
		y,
		width: NODE_W,
		height: NODE_H,
		dto: nodes.get(id)!
	}));
}

function modulatorsForChain(snapshot: GraphSnapshotDto, chainIds: Set<number>): GraphNodeDto[] {
	const modIds = new Set<number>();
	for (const e of snapshot.modEdges) {
		if (chainIds.has(e.targetNode)) modIds.add(e.sourceNode);
	}
	return snapshot.nodes.filter((n) => n.kind === 'modulator' && modIds.has(n.id));
}

function placeModulators(
	mods: GraphNodeDto[],
	snapshot: GraphSnapshotDto,
	layoutNodes: LayoutNode[],
	baseY: number
): LayoutNode[] {
	const byId = new Map(layoutNodes.map((n) => [n.id, n]));
	const placed: LayoutNode[] = [];
	let modIndex = 0;

	for (const mod of mods) {
		const targets = snapshot.modEdges
			.filter((e) => e.sourceNode === mod.id)
			.map((e) => byId.get(e.targetNode))
			.filter(Boolean) as LayoutNode[];

		const anchor = targets[0];
		const x = anchor ? anchor.x : PADDING + modIndex * (NODE_W + H_GAP);
		const y = baseY + LFO_OFFSET_Y;

		placed.push({
			id: mod.id,
			x,
			y,
			width: NODE_W,
			height: NODE_H,
			dto: mod
		});
		modIndex++;
	}
	return placed;
}

function layoutBounds(nodes: LayoutNode[]): { width: number; height: number } {
	if (nodes.length === 0) {
		return { width: 400, height: 200 };
	}
	let maxX = 0;
	let maxY = 0;
	for (const n of nodes) {
		maxX = Math.max(maxX, n.x + n.width);
		maxY = Math.max(maxY, n.y + n.height);
	}
	return { width: maxX + PADDING, height: maxY + PADDING };
}

/** Single-track signal chain + LFOs. */
export function layoutTrackGraph(snapshot: GraphSnapshotDto, trackId: string): GraphLayout {
	const nodes = nodeMap(snapshot);
	const adj = audioAdjacency(snapshot.audioEdges);

	const source = snapshot.nodes.find(
		(n) => n.trackId === trackId && (n.kind === 'track' || n.kind === 'instrument')
	);
	if (!source) {
		return { nodes: [], width: 400, height: 200 };
	}

	const chainIds = chainFromSource(source.id, adj, nodes);
	const chainSet = new Set(chainIds);
	const mainRow = placeRow(chainIds, nodes, PADDING);

	const mods = modulatorsForChain(snapshot, chainSet);
	const modRow = placeModulators(mods, snapshot, mainRow, PADDING);

	const all = [...mainRow, ...modRow];
	const bounds = layoutBounds(all);
	return { nodes: all, ...bounds };
}

/** Full project: one row per track converging to Master. */
export function layoutProjectGraph(snapshot: GraphSnapshotDto): GraphLayout {
	const nodes = nodeMap(snapshot);
	const adj = audioAdjacency(snapshot.audioEdges);
	const master = snapshot.nodes.find((n) => n.kind === 'master');

	const trackIds = [
		...new Set(
			snapshot.nodes
				.filter((n) => n.trackId && (n.kind === 'track' || n.kind === 'instrument'))
				.map((n) => n.trackId!)
		)
	];

	const layoutNodes: LayoutNode[] = [];
	let maxChainLen = 0;

	trackIds.forEach((trackId, row) => {
		const source = snapshot.nodes.find(
			(n) => n.trackId === trackId && (n.kind === 'track' || n.kind === 'instrument')
		);
		if (!source) return;

		const chainIds = chainFromSource(source.id, adj, nodes);
		maxChainLen = Math.max(maxChainLen, chainIds.length);
		const chainSet = new Set(chainIds);
		const y = PADDING + row * TRACK_ROW_H;
		const rowNodes = placeRow(chainIds, nodes, y);
		layoutNodes.push(...rowNodes);

		const mods = modulatorsForChain(snapshot, chainSet);
		layoutNodes.push(...placeModulators(mods, snapshot, rowNodes, y));
	});

	if (master) {
		const masterX = PADDING + maxChainLen * (NODE_W + H_GAP) + H_GAP;
		const masterY =
			trackIds.length > 0 ? PADDING + ((trackIds.length - 1) * TRACK_ROW_H) / 2 : PADDING;

		layoutNodes.push({
			id: master.id,
			x: masterX,
			y: masterY,
			width: NODE_W + 20,
			height: NODE_H + 8,
			dto: master
		});
	}

	const bounds = layoutBounds(layoutNodes);
	return { nodes: layoutNodes, ...bounds };
}

/** Filter snapshot to nodes visible in a layout. */
export function visibleNodeIds(layout: GraphLayout): Set<number> {
	return new Set(layout.nodes.map((n) => n.id));
}

export function visibleAudioEdges(snapshot: GraphSnapshotDto, layout: GraphLayout): AudioEdgeDto[] {
	const ids = visibleNodeIds(layout);
	return snapshot.audioEdges.filter((e) => ids.has(e.fromNode) && ids.has(e.toNode));
}

export function visibleModEdges(snapshot: GraphSnapshotDto, layout: GraphLayout): ModEdgeDto[] {
	const ids = visibleNodeIds(layout);
	return snapshot.modEdges.filter((e) => ids.has(e.sourceNode) && ids.has(e.targetNode));
}
