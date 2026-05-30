/** Mirror of Rust `GraphSnapshotDto` (camelCase JSON). */

export interface GraphNodeDto {
	id: number;
	kind: string;
	label: string;
	trackId: string | null;
	modulatorId: string | null;
	effectKind: string | null;
	summary: string | null;
}

export interface AudioEdgeDto {
	fromNode: number;
	fromPort: number;
	toNode: number;
	toPort: number;
}

export interface ModEdgeDto {
	sourceNode: number;
	sourcePort: number;
	targetNode: number;
	param: string;
	depth: number;
	routeId: string | null;
}

export interface GraphSnapshotDto {
	nodes: GraphNodeDto[];
	audioEdges: AudioEdgeDto[];
	modEdges: ModEdgeDto[];
}

export interface LayoutNode {
	id: number;
	x: number;
	y: number;
	width: number;
	height: number;
	dto: GraphNodeDto;
}

export interface GraphLayout {
	nodes: LayoutNode[];
	width: number;
	height: number;
}

export const NODE_W = 140;
export const NODE_H = 60;
export const H_GAP = 52;
export const TRACK_ROW_H = 120;
export const LFO_OFFSET_Y = 78;
export const PADDING = 48;

export const KIND_LABELS: Record<string, string> = {
	track: 'Clips',
	instrument: 'Instrument',
	track_gain: 'Volume',
	meter: 'Meter',
	master: 'Master',
	modulator: 'LFO',
	eq: 'EQ',
	compressor: 'Comp',
	reverb: 'Réverb',
	delay: 'Délai',
	gain: 'Gain',
	pan: 'Pan',
	bus: 'Bus',
	send: 'Send',
	return: 'Return',
	plugin: 'Plugin',
};
