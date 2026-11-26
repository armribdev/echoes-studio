import type { MidiControlPoint } from '../../types/global';

/** A control the piano roll can draw as a lane. Polyphonic pressure has none: it follows its notes. */
export interface ControlLane {
	kind: 'pitch_bend' | 'control_change' | 'channel_pressure';
	number: number;
}

export interface LanePoint {
	beat: number;
	value: number;
}

/** Default and minimum height: the picker and buttons of the keys column need it. */
export const LANE_HEIGHT = 52;
export const MAX_LANE_HEIGHT = 160;
export const ADD_ROW_HEIGHT = 22;
export const MAX_VISIBLE_LANES = 3;

const LANE_PADDING_PX = 5;
const BEND_MAX = 16383;
const BEND_CENTER = 8192;
const SEVEN_BIT_MAX = 127;

export const PITCH_BEND: ControlLane = { kind: 'pitch_bend', number: 0 };
export const MODULATION: ControlLane = { kind: 'control_change', number: 1 };
export const EXPRESSION: ControlLane = { kind: 'control_change', number: 11 };
export const SUSTAIN: ControlLane = { kind: 'control_change', number: 64 };
export const AFTERTOUCH: ControlLane = { kind: 'channel_pressure', number: 0 };

export const LANE_PRESETS: ControlLane[] = [
	PITCH_BEND,
	MODULATION,
	SUSTAIN,
	EXPRESSION,
	AFTERTOUCH
];

/** Controllers 120-127 are channel-mode messages (all notes off…), not something to draw. */
const DRAWABLE_CONTROLLERS = 120;

export function clampLaneHeight(heightPx: number): number {
	return Math.min(MAX_LANE_HEIGHT, Math.max(LANE_HEIGHT, Math.round(heightPx)));
}

export function laneKey(lane: ControlLane): string {
	return `${lane.kind}:${lane.number}`;
}

export function sameLane(a: ControlLane, b: ControlLane): boolean {
	return a.kind === b.kind && a.number === b.number;
}

export function laneOf(point: MidiControlPoint): ControlLane | null {
	if (point.kind === 'poly_pressure') return null;
	return { kind: point.kind, number: point.kind === 'control_change' ? point.number : 0 };
}

export function laneMax(lane: ControlLane): number {
	return lane.kind === 'pitch_bend' ? BEND_MAX : SEVEN_BIT_MAX;
}

/** The value a control returns to outside a clip; `null` when it keeps its level (volume, filters…). */
export function laneNeutral(lane: ControlLane): number | null {
	if (lane.kind === 'pitch_bend') return BEND_CENTER;
	if (lane.kind === 'channel_pressure') return 0;
	const neutralControllers: Record<number, number> = { 1: 0, 11: 127, 64: 0, 65: 0, 66: 0, 67: 0 };
	return neutralControllers[lane.number] ?? null;
}

/** Presets first, then the other controllers by number. */
export function lanesWithData(controls: MidiControlPoint[]): ControlLane[] {
	const found = new Map<string, ControlLane>();
	for (const point of controls) {
		const lane = laneOf(point);
		if (lane) found.set(laneKey(lane), lane);
	}
	const presets = LANE_PRESETS.filter((lane) => found.has(laneKey(lane)));
	const others = [...found.values()]
		.filter((lane) => !LANE_PRESETS.some((preset) => sameLane(preset, lane)))
		.sort((a, b) => a.number - b.number);
	return [...presets, ...others];
}

/** What the lane pickers offer: presets, lanes holding data, then every other controller by number. */
export function candidateLanes(controls: MidiControlPoint[], shown: ControlLane[]): ControlLane[] {
	const everyController = Array.from({ length: DRAWABLE_CONTROLLERS }, (_, number) => ({
		kind: 'control_change' as const,
		number
	}));
	return [...LANE_PRESETS, ...lanesWithData(controls), ...everyController].filter(
		(lane, index, all) =>
			all.findIndex((other) => sameLane(other, lane)) === index &&
			!shown.some((shownLane) => sameLane(shownLane, lane))
	);
}

export function lanePoints(controls: MidiControlPoint[], lane: ControlLane): MidiControlPoint[] {
	return controls.filter((point) => {
		const pointLane = laneOf(point);
		return pointLane !== null && sameLane(pointLane, lane);
	});
}

/** Swaps one lane's points; the other controls are kept as they are. */
export function replaceLanePoints(
	controls: MidiControlPoint[],
	lane: ControlLane,
	points: MidiControlPoint[]
): MidiControlPoint[] {
	const others = controls.filter((point) => {
		const pointLane = laneOf(point);
		return pointLane === null || !sameLane(pointLane, lane);
	});
	return [...others, ...points].sort((a, b) => a.start - b.start);
}

export function toControlPoints(lane: ControlLane, points: LanePoint[]): MidiControlPoint[] {
	return points.map((point) => ({
		start: point.beat,
		kind: lane.kind,
		number: lane.number,
		value: point.value
	}));
}

export function toLanePoints(points: MidiControlPoint[]): LanePoint[] {
	return points.map((point) => ({ beat: point.start, value: point.value }));
}

/** A bend reads as a signed offset from the centre, the others as plain numbers. */
export function formatLaneValue(lane: ControlLane, value: number): string {
	if (lane.kind !== 'pitch_bend') return String(value);
	const offset = value - BEND_CENTER;
	return offset > 0 ? `+${offset}` : String(offset);
}

export function valueToY(value: number, lane: ControlLane, heightPx: number): number {
	const usable = heightPx - 2 * LANE_PADDING_PX;
	return LANE_PADDING_PX + (1 - value / laneMax(lane)) * usable;
}

export function yToValue(y: number, lane: ControlLane, heightPx: number): number {
	const usable = heightPx - 2 * LANE_PADDING_PX;
	const ratio = 1 - (y - LANE_PADDING_PX) / usable;
	return Math.round(Math.min(1, Math.max(0, ratio)) * laneMax(lane));
}

/** The curve as the engine plays it: each value holds until the next point. */
export function stepPath(
	points: LanePoint[],
	lane: ControlLane,
	pxPerBeat: number,
	widthPx: number,
	heightPx: number
): string {
	if (points.length === 0) return '';
	const neutral = laneNeutral(lane);
	const first = points[0];
	let path =
		neutral !== null && first.beat > 0
			? `M 0 ${valueToY(neutral, lane, heightPx)} H ${first.beat * pxPerBeat}`
			: `M ${first.beat * pxPerBeat} ${valueToY(first.value, lane, heightPx)}`;
	path += ` V ${valueToY(first.value, lane, heightPx)}`;
	for (const point of points.slice(1)) {
		path += ` H ${point.beat * pxPerBeat} V ${valueToY(point.value, lane, heightPx)}`;
	}
	const lastX = points[points.length - 1].beat * pxPerBeat;
	return `${path} H ${Math.max(widthPx, lastX)}`;
}

/** Values of a straight line between two points, one per `stepBeats`, as a held-value staircase. */
export function rampPoints(from: LanePoint, to: LanePoint, stepBeats: number): LanePoint[] {
	const [start, end] = from.beat <= to.beat ? [from, to] : [to, from];
	const span = end.beat - start.beat;
	if (span <= 0 || stepBeats <= 0) return [{ beat: end.beat, value: end.value }];
	const steps = Math.max(1, Math.round(span / stepBeats));
	const ramp: LanePoint[] = [];
	for (let step = 0; step <= steps; step++) {
		const t = step / steps;
		ramp.push({
			beat: start.beat + t * span,
			value: Math.round(start.value + t * (end.value - start.value))
		});
	}
	return dropRepeatedValues(ramp);
}

/** Free-hand samples reduced to one value per `stepBeats` (the last one), without repeats. */
export function strokePoints(samples: LanePoint[], stepBeats: number): LanePoint[] {
	const buckets = new Map<number, LanePoint>();
	for (const sample of samples) {
		buckets.set(Math.floor(sample.beat / stepBeats), sample);
	}
	const stepped = [...buckets.entries()]
		.sort((a, b) => a[0] - b[0])
		.map(([bucket, sample]) => ({ beat: bucket * stepBeats, value: sample.value }));
	return dropRepeatedValues(stepped);
}

/** `added` replaces whatever `points` held between its first and last beat. */
export function replaceSpan(points: LanePoint[], added: LanePoint[]): LanePoint[] {
	if (added.length === 0) return points;
	const from = added[0].beat;
	const to = added[added.length - 1].beat;
	const kept = points.filter((point) => point.beat < from || point.beat > to);
	return [...kept, ...added].sort((a, b) => a.beat - b.beat);
}

function dropRepeatedValues(points: LanePoint[]): LanePoint[] {
	return points.filter((point, index) => index === 0 || point.value !== points[index - 1].value);
}
