import type {
	AutomationClip,
	AutomationLane,
	AutomationPoint,
	InterpolationMode,
	ModulationTargetRef,
	TrackEffect
} from '../../types/global';
import { effectGroupNames, internalParam, type PluginParamInfo } from '../modulation/targets';

/** Vertical inset so points at 0 and 1 stay fully visible. */
export const LANE_PADDING_PX = 6;

export function lanePoints(lane: AutomationLane): AutomationPoint[] {
	return lane.clips.flatMap((clip) => clip.points).sort((a, b) => a.time_s - b.time_s);
}

/**
 * The UI edits a lane as one clip spanning the timeline start to its last point:
 * the engine drops points outside their clip window.
 */
export function laneClipFor(lane: AutomationLane, points: AutomationPoint[]): AutomationClip {
	const sorted = [...points].sort((a, b) => a.time_s - b.time_s);
	return {
		id: lane.clips[0]?.id ?? crypto.randomUUID(),
		start_s: 0,
		end_s: sorted.length > 0 ? sorted[sorted.length - 1].time_s : 0,
		points: sorted
	};
}

export type AutomationShape = 'ramp_up' | 'ramp_down' | 'sine';

/** Sampled shapes (sine, and ramps on a step lane) use this many segments. */
const SHAPE_SEGMENTS = 16;

function shapeValue(shape: AutomationShape, t: number): number {
	switch (shape) {
		case 'ramp_up':
			return t;
		case 'ramp_down':
			return 1 - t;
		case 'sine':
			return 0.5 - 0.5 * Math.cos(2 * Math.PI * t);
	}
}

/** Points of `shape` over [`fromS`, `toS`], on the full 0..1 range. */
export function shapePoints(
	shape: AutomationShape,
	fromS: number,
	toS: number,
	interpolation: InterpolationMode
): AutomationPoint[] {
	const segments = shape === 'sine' || interpolation === 'step' ? SHAPE_SEGMENTS : 1;
	return Array.from({ length: segments + 1 }, (_, i) => {
		const t = i / segments;
		return { time_s: fromS + t * (toS - fromS), value: shapeValue(shape, t) };
	});
}

/** The shape replaces the points under its span; the rest of the lane is kept. */
export function withShape(
	points: AutomationPoint[],
	shape: AutomationShape,
	fromS: number,
	toS: number,
	interpolation: InterpolationMode
): AutomationPoint[] {
	const kept = points.filter((p) => p.time_s < fromS || p.time_s > toS);
	return [...kept, ...shapePoints(shape, fromS, toS, interpolation)].sort(
		(a, b) => a.time_s - b.time_s
	);
}

/** Same key as the matching `ModulationTargetOption` (`targets.ts`). */
export function targetKey(target: ModulationTargetRef): string {
	switch (target.kind) {
		case 'track_gain':
			return 'track_gain';
		case 'effect_param':
			return `${target.effect_id}/${target.param}`;
		case 'plugin_param':
			return `${target.effect_id}/${target.native_id}`;
	}
}

export function valueToY(value: number, heightPx: number): number {
	return LANE_PADDING_PX + (1 - value) * (heightPx - 2 * LANE_PADDING_PX);
}

export function yToValue(y: number, heightPx: number): number {
	const value = 1 - (y - LANE_PADDING_PX) / (heightPx - 2 * LANE_PADDING_PX);
	return Math.min(1, Math.max(0, value));
}

/** SVG path of the curve as the engine plays it: held flat before the first and after the last point. */
export function curvePath(
	points: AutomationPoint[],
	interpolation: InterpolationMode,
	scale: number,
	widthPx: number,
	heightPx: number
): string {
	if (points.length === 0) return '';
	const xy = points.map((p) => [p.time_s * scale, valueToY(p.value, heightPx)] as const);
	let path = `M 0 ${xy[0][1]} L ${xy[0][0]} ${xy[0][1]}`;
	for (let i = 1; i < xy.length; i++) {
		const [x, y] = xy[i];
		path += interpolation === 'step' ? ` H ${x} V ${y}` : ` L ${x} ${y}`;
	}
	return `${path} H ${Math.max(widthPx, xy[xy.length - 1][0])}`;
}

export function laneLabel(
	target: ModulationTargetRef,
	effects: TrackEffect[],
	instrumentName: string | null,
	paramsByPlacement: Record<string, PluginParamInfo[]>
): { group: string; name: string } {
	switch (target.kind) {
		case 'track_gain':
			return { group: 'Piste', name: 'Volume' };
		case 'effect_param': {
			const effect = effects.find((e) => e.id === target.effect_id);
			return {
				group: effectGroupNames(effects).get(target.effect_id) ?? 'Effet',
				name: internalParam(effect, target.param)?.label ?? target.param
			};
		}
		case 'plugin_param': {
			const param = targetPluginParam(target, paramsByPlacement);
			return {
				group: effectGroupNames(effects).get(target.effect_id) ?? instrumentName ?? 'Plugin',
				name: param?.name ?? `Réglage ${target.native_id}`
			};
		}
	}
}

export function targetPluginParam(
	target: ModulationTargetRef,
	paramsByPlacement: Record<string, PluginParamInfo[]>
): PluginParamInfo | undefined {
	if (target.kind !== 'plugin_param') return undefined;
	return paramsByPlacement[target.effect_id]?.find((p) => p.native_id === target.native_id);
}

/** How a lane's 0..1 value maps to the parameter's own unit, mirroring the engine (ADR-025). */
export interface AutomationRange {
	min: number;
	max: number;
	log: boolean;
	/** `%` displays the value x100. */
	unit: string;
}

const VOLUME_RANGE: AutomationRange = { min: 0, max: 2, log: false, unit: '%' };

export function automationRange(
	target: ModulationTargetRef,
	effects: TrackEffect[],
	paramsByPlacement: Record<string, PluginParamInfo[]>
): AutomationRange | undefined {
	switch (target.kind) {
		case 'track_gain':
			return VOLUME_RANGE;
		case 'effect_param': {
			const param = internalParam(
				effects.find((e) => e.id === target.effect_id),
				target.param
			);
			return param && { min: param.min, max: param.max, log: param.log === true, unit: param.unit };
		}
		case 'plugin_param': {
			const param = targetPluginParam(target, paramsByPlacement);
			return (
				param && {
					min: param.min,
					max: param.max,
					log: false,
					unit: param.min === 0 && param.max === 1 ? '%' : ''
				}
			);
		}
	}
}

/** The parameter's own value for a 0..1 lane value, on the engine's range and scale. */
export function toPlainValue(value: number, range: AutomationRange): number {
	return range.log
		? range.min * (range.max / range.min) ** value
		: range.min + value * (range.max - range.min);
}

/** Lane value at `timeS`, as the engine evaluates it: held before the first and after the last point. */
export function automationValueAt(lane: AutomationLane, timeS: number): number | undefined {
	const points = lanePoints(lane);
	if (points.length === 0) return undefined;
	const after = points.findIndex((p) => p.time_s > timeS);
	if (after === 0) return points[0].value;
	if (after === -1) return points[points.length - 1].value;
	const previous = points[after - 1];
	const next = points[after];
	const span = next.time_s - previous.time_s;
	if (lane.interpolation === 'step' || span < 1e-10)
		return lane.interpolation === 'step' ? previous.value : next.value;
	return previous.value + ((timeS - previous.time_s) / span) * (next.value - previous.value);
}

export function formatAutomationValue(value: number, range: AutomationRange | undefined): string {
	if (!range) return `${Math.round(value * 100)} %`;
	const plain = toPlainValue(value, range);
	if (range.unit === '%') return `${Math.round(plain * 100)} %`;
	if (range.unit === 'Hz' && plain >= 1000) return `${(plain / 1000).toFixed(2)} kHz`;
	const span = Math.abs(range.max - range.min);
	const digits = range.unit === 'dB' ? 1 : span >= 100 ? 0 : span >= 10 ? 1 : 2;
	return `${plain.toFixed(digits)}${range.unit ? ` ${range.unit}` : ''}`;
}

interface PixelPoint {
	x: number;
	y: number;
}

/** Ramer–Douglas–Peucker: drops points closer than `epsilonPx` to the line through their neighbours. */
export function simplifyPolyline<T extends PixelPoint>(points: T[], epsilonPx: number): T[] {
	if (points.length <= 2) return points;
	const first = points[0];
	const last = points[points.length - 1];
	let farthestIndex = 0;
	let farthestDistance = 0;
	for (let i = 1; i < points.length - 1; i++) {
		const distance = distanceToLine(points[i], first, last);
		if (distance > farthestDistance) {
			farthestDistance = distance;
			farthestIndex = i;
		}
	}
	if (farthestDistance <= epsilonPx) return [first, last];
	const left = simplifyPolyline(points.slice(0, farthestIndex + 1), epsilonPx);
	const right = simplifyPolyline(points.slice(farthestIndex), epsilonPx);
	return [...left.slice(0, -1), ...right];
}

function distanceToLine(point: PixelPoint, start: PixelPoint, end: PixelPoint): number {
	const dx = end.x - start.x;
	const dy = end.y - start.y;
	const length = Math.hypot(dx, dy);
	if (length === 0) return Math.hypot(point.x - start.x, point.y - start.y);
	return Math.abs(dy * point.x - dx * point.y + end.x * start.y - end.y * start.x) / length;
}
