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
      const param = paramsByPlacement[target.effect_id]?.find((p) => p.native_id === target.native_id);
      return {
        group: effectGroupNames(effects).get(target.effect_id) ?? instrumentName ?? 'Plugin',
        name: param?.name ?? `Réglage ${target.native_id}`,
      };
    }
  }
}
