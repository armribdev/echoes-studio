import type {
  AutomationClip,
  AutomationLane,
  AutomationPoint,
  InterpolationMode,
  ModulationTargetRef,
  TrackEffect,
} from '../../types/global';
import { effectGroupNames, type PluginParamInfo } from '../modulation/targets';

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
    points: sorted,
  };
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
  heightPx: number,
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
  paramsByPlacement: Record<string, PluginParamInfo[]>,
): { group: string; name: string } {
  switch (target.kind) {
    case 'track_gain':
      return { group: 'Piste', name: 'Volume' };
    case 'effect_param':
      return { group: effectGroupNames(effects).get(target.effect_id) ?? 'Effet', name: target.param };
    case 'plugin_param': {
      const param = targetPluginParam(target, paramsByPlacement);
      return {
        group: effectGroupNames(effects).get(target.effect_id) ?? instrumentName ?? 'Plugin',
        name: param?.name ?? `Réglage ${target.native_id}`,
      };
    }
  }
}

export function targetPluginParam(
  target: ModulationTargetRef,
  paramsByPlacement: Record<string, PluginParamInfo[]>,
): PluginParamInfo | undefined {
  if (target.kind !== 'plugin_param') return undefined;
  return paramsByPlacement[target.effect_id]?.find((p) => p.native_id === target.native_id);
}

/** Plain value in the plugin's own range, or a percentage when the range is 0…1 or unknown. */
export function formatAutomationValue(value: number, param: PluginParamInfo | undefined): string {
  if (!param || (param.min === 0 && param.max === 1)) return `${Math.round(value * 100)} %`;
  const plain = param.min + value * (param.max - param.min);
  const span = Math.abs(param.max - param.min);
  return plain.toFixed(span >= 100 ? 0 : span >= 10 ? 1 : 2);
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
