import type {
	AudioEdgeDto,
	GraphLayout,
	LayoutNode,
	ModEdgeDto,
} from './types';
import { KIND_LABELS, NODE_H, NODE_W } from './types';

export interface GraphRenderOptions {
	trackColor: string;
	hoveredNodeId: number | null;
	selectedNodeId: number | null;
	modPhase: number;
	dpr: number;
}

const COLORS = {
	bg: '#1a1a1f',
	grid: 'rgba(255,255,255,0.04)',
	nodeBg: '#222229',
	nodeBorder: 'rgba(255,255,255,0.12)',
	nodeHover: 'rgba(21,113,255,0.35)',
	text: '#e8e8ec',
	textMuted: '#888894',
	mod: '#ff9a3c',
	masterAccent: '#6bcb77',
	audioCable: 'rgba(255,255,255,0.35)',
};

function portOut(ln: LayoutNode): { x: number; y: number } {
	return { x: ln.x + ln.width, y: ln.y + ln.height / 2 };
}

function portIn(ln: LayoutNode): { x: number; y: number } {
	return { x: ln.x, y: ln.y + ln.height / 2 };
}

function modPortOut(ln: LayoutNode): { x: number; y: number } {
	return { x: ln.x + ln.width / 2, y: ln.y };
}

function modPortIn(ln: LayoutNode): { x: number; y: number } {
	return { x: ln.x + ln.width / 2, y: ln.y + ln.height };
}

function drawBezier(
	ctx: CanvasRenderingContext2D,
	x1: number,
	y1: number,
	x2: number,
	y2: number,
) {
	const dx = Math.abs(x2 - x1) * 0.5;
	ctx.moveTo(x1, y1);
	ctx.bezierCurveTo(x1 + dx, y1, x2 - dx, y2, x2, y2);
}

function drawGrid(ctx: CanvasRenderingContext2D, w: number, h: number) {
	ctx.fillStyle = COLORS.bg;
	ctx.fillRect(0, 0, w, h);

	const step = 24;
	ctx.fillStyle = COLORS.grid;
	for (let x = 0; x < w; x += step) {
		for (let y = 0; y < h; y += step) {
			ctx.fillRect(x, y, 1, 1);
		}
	}
}

function displayLabel(kind: string, label: string): string {
	const short = KIND_LABELS[kind] ?? kind;
	if (kind === 'modulator') {
		const m = label.match(/\[mod:([^\]]+)\]/);
		return m ? `LFO` : short;
	}
	return short;
}

function drawNode(
	ctx: CanvasRenderingContext2D,
	ln: LayoutNode,
	opts: GraphRenderOptions,
) {
	const { trackColor, hoveredNodeId, selectedNodeId } = opts;
	const isMaster = ln.dto.kind === 'master';
	const isMod = ln.dto.kind === 'modulator';
	const isHovered = ln.id === hoveredNodeId;
	const isSelected = ln.id === selectedNodeId;
	const w = ln.width;
	const h = ln.height;

	ctx.fillStyle = COLORS.nodeBg;
	ctx.strokeStyle = isHovered || isSelected ? COLORS.nodeHover : COLORS.nodeBorder;
	ctx.lineWidth = isSelected ? 2 : 1;
	ctx.beginPath();
	ctx.roundRect(ln.x, ln.y, w, h, 6);
	ctx.fill();
	ctx.stroke();

	// Accent bar
	ctx.fillStyle = isMaster ? COLORS.masterAccent : isMod ? COLORS.mod : trackColor;
	ctx.beginPath();
	ctx.roundRect(ln.x, ln.y, 4, h, [6, 0, 0, 6]);
	ctx.fill();

	// Header
	ctx.fillStyle = COLORS.text;
	ctx.font = '600 11px system-ui, sans-serif';
	ctx.textAlign = 'left';
	ctx.textBaseline = 'top';
	ctx.fillText(displayLabel(ln.dto.kind, ln.dto.label), ln.x + 10, ln.y + 8);

	if (ln.dto.summary) {
		ctx.fillStyle = COLORS.textMuted;
		ctx.font = '9px "JetBrains Mono", monospace';
		ctx.fillText(ln.dto.summary, ln.x + 10, ln.y + 26);
	}

	// Audio ports
	if (!isMod) {
		if (ln.dto.kind !== 'track' && ln.dto.kind !== 'instrument') {
			ctx.fillStyle = '#ccc';
			ctx.beginPath();
			ctx.arc(ln.x, ln.y + h / 2, 4, 0, Math.PI * 2);
			ctx.fill();
		}
		if (ln.dto.kind !== 'meter' && ln.dto.kind !== 'master') {
			ctx.fillStyle = '#ccc';
			ctx.beginPath();
			ctx.arc(ln.x + w, ln.y + h / 2, 4, 0, Math.PI * 2);
			ctx.fill();
		}
	} else {
		// Mod out port (top)
		ctx.fillStyle = COLORS.mod;
		ctx.beginPath();
		ctx.moveTo(ln.x + w / 2, ln.y - 5);
		ctx.lineTo(ln.x + w / 2 + 5, ln.y);
		ctx.lineTo(ln.x + w / 2 - 5, ln.y);
		ctx.closePath();
		ctx.fill();
	}

	// Mod in port (bottom) for modulatable nodes
	if (
		ln.dto.kind === 'track_gain' ||
		ln.dto.effectKind !== null
	) {
		ctx.fillStyle = COLORS.mod;
		ctx.globalAlpha = 0.7;
		ctx.beginPath();
		ctx.moveTo(ln.x + w / 2, ln.y + h + 5);
		ctx.lineTo(ln.x + w / 2 + 5, ln.y + h);
		ctx.lineTo(ln.x + w / 2 - 5, ln.y + h);
		ctx.closePath();
		ctx.fill();
		ctx.globalAlpha = 1;
	}
}

function drawAudioEdge(
	ctx: CanvasRenderingContext2D,
	edge: AudioEdgeDto,
	byId: Map<number, LayoutNode>,
	trackColor: string,
) {
	const from = byId.get(edge.fromNode);
	const to = byId.get(edge.toNode);
	if (!from || !to) return;

	const p0 = portOut(from);
	const p1 = portIn(to);

	ctx.strokeStyle = trackColor;
	ctx.globalAlpha = 0.55;
	ctx.lineWidth = 2.5;
	ctx.beginPath();
	drawBezier(ctx, p0.x, p0.y, p1.x, p1.y);
	ctx.stroke();
	ctx.globalAlpha = 1;
}

function drawModEdge(
	ctx: CanvasRenderingContext2D,
	edge: ModEdgeDto,
	byId: Map<number, LayoutNode>,
	modPhase: number,
) {
	const from = byId.get(edge.sourceNode);
	const to = byId.get(edge.targetNode);
	if (!from || !to) return;

	const p0 = modPortOut(from);
	const p1 = modPortIn(to);

	ctx.strokeStyle = COLORS.mod;
	ctx.globalAlpha = 0.45 + 0.25 * Math.sin(modPhase);
	ctx.lineWidth = 1.5;
	ctx.setLineDash([4, 4]);
	ctx.beginPath();
	drawBezier(ctx, p0.x, p0.y, p1.x, p1.y);
	ctx.stroke();
	ctx.setLineDash([]);

	// Depth label at midpoint
	const mx = (p0.x + p1.x) / 2;
	const my = (p0.y + p1.y) / 2;
	ctx.globalAlpha = 0.85;
	ctx.fillStyle = COLORS.mod;
	ctx.font = '8px "JetBrains Mono", monospace';
	ctx.textAlign = 'center';
	ctx.fillText(`${edge.param} ${edge.depth >= 0 ? '+' : ''}${edge.depth.toFixed(2)}`, mx, my - 4);
	ctx.globalAlpha = 1;
}

export function renderGraph(
	ctx: CanvasRenderingContext2D,
	layout: GraphLayout,
	audioEdges: AudioEdgeDto[],
	modEdges: ModEdgeDto[],
	opts: GraphRenderOptions,
	canvasW: number,
	canvasH: number,
) {
	drawGrid(ctx, canvasW, canvasH);

	const byId = new Map(layout.nodes.map((n) => [n.id, n]));

	for (const e of audioEdges) {
		drawAudioEdge(ctx, e, byId, opts.trackColor);
	}
	for (const e of modEdges) {
		drawModEdge(ctx, e, byId, opts.modPhase);
	}
	for (const ln of layout.nodes) {
		drawNode(ctx, ln, opts);
	}
}

export function hitTestNode(
	layout: GraphLayout,
	x: number,
	y: number,
): LayoutNode | null {
	for (let i = layout.nodes.length - 1; i >= 0; i--) {
		const n = layout.nodes[i];
		if (x >= n.x && x <= n.x + n.width && y >= n.y && y <= n.y + n.height) {
			return n;
		}
	}
	return null;
}
