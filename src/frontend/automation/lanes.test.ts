import { describe, expect, it } from 'vitest';
import type { AutomationLane, AutomationPoint } from '../../types/global';
import {
	curvePath,
	laneClipFor,
	lanePoints,
	shapePoints,
	targetKey,
	valueToY,
	withShape,
	yToValue
} from './lanes';

const points: AutomationPoint[] = [
	{ time_s: 1, value: 0.25 },
	{ time_s: 0, value: 0 },
	{ time_s: 2, value: 1 }
];

describe('lanePoints / laneClipFor', () => {
	it('flattens and sorts points across clips', () => {
		const lane: AutomationLane = {
			id: 'l1',
			target: { kind: 'track_gain', track_id: 't1' },
			interpolation: 'linear',
			clips: [
				{ id: 'a', start_s: 0, end_s: 1, points: [points[1], points[0]] },
				{ id: 'b', start_s: 1, end_s: 2, points: [points[2]] }
			]
		};
		expect(lanePoints(lane).map((p) => p.time_s)).toEqual([0, 1, 2]);
	});

	it('builds a single clip spanning from 0 to the last point', () => {
		const lane: AutomationLane = {
			id: 'l1',
			target: { kind: 'track_gain', track_id: 't1' },
			interpolation: 'linear',
			clips: [{ id: 'keep', start_s: 0, end_s: 0, points: [] }]
		};
		const clip = laneClipFor(lane, points);
		expect(clip.id).toBe('keep');
		expect(clip.start_s).toBe(0);
		expect(clip.end_s).toBe(2);
		expect(clip.points.map((p) => p.time_s)).toEqual([0, 1, 2]);
	});
});

describe('valueToY / yToValue', () => {
	it('round-trips values with lane padding', () => {
		const height = 100;
		for (const value of [0, 0.5, 1]) {
			expect(yToValue(valueToY(value, height), height)).toBeCloseTo(value, 10);
		}
		expect(yToValue(-10, height)).toBe(1);
		expect(yToValue(999, height)).toBe(0);
	});
});

describe('curvePath / targetKey', () => {
	it('builds linear and step SVG paths', () => {
		const linear = curvePath(points, 'linear', 10, 40, 100);
		expect(linear.startsWith('M 0 ')).toBe(true);
		expect(linear).toContain(' L ');
		expect(linear.endsWith(' H 40') || linear.includes(' H 40')).toBe(true);

		const step = curvePath(
			[
				{ time_s: 0, value: 0 },
				{ time_s: 1, value: 1 }
			],
			'step',
			10,
			20,
			100
		);
		expect(step).toContain(' H ');
		expect(step).toContain(' V ');
	});

	it('returns an empty path for no points', () => {
		expect(curvePath([], 'linear', 1, 10, 10)).toBe('');
	});

	it('keys modulation targets stably', () => {
		expect(targetKey({ kind: 'track_gain', track_id: 't1' })).toBe('track_gain');
		expect(
			targetKey({ kind: 'effect_param', track_id: 't1', effect_id: 'e1', param: 'freq' })
		).toBe('e1/freq');
		expect(targetKey({ kind: 'plugin_param', effect_id: 'e1', native_id: 3 })).toBe('e1/3');
	});
});

describe('shapePoints / withShape', () => {
	it('draws a linear ramp with its two ends', () => {
		expect(shapePoints('ramp_up', 2, 6, 'linear')).toEqual([
			{ time_s: 2, value: 0 },
			{ time_s: 6, value: 1 }
		]);
		expect(shapePoints('ramp_down', 2, 6, 'linear').map((p) => p.value)).toEqual([1, 0]);
	});

	it('samples a ramp on a step lane so it climbs by steps', () => {
		const steps = shapePoints('ramp_up', 0, 4, 'step');
		expect(steps).toHaveLength(17);
		expect(steps[8].value).toBeCloseTo(0.5);
	});

	it('makes one sine cycle from the minimum, peaking at mid-span', () => {
		const sine = shapePoints('sine', 0, 8, 'linear');
		expect(sine[0].value).toBeCloseTo(0);
		expect(sine[8].value).toBeCloseTo(1);
		expect(sine[16].value).toBeCloseTo(0);
		expect(sine[16].time_s).toBe(8);
	});

	it('replaces the points under the span and keeps the others', () => {
		const lane = [
			{ time_s: 0, value: 0.3 },
			{ time_s: 3, value: 0.9 },
			{ time_s: 9, value: 0.1 }
		];
		const next = withShape(lane, 'ramp_up', 2, 6, 'linear');
		expect(next).toEqual([
			{ time_s: 0, value: 0.3 },
			{ time_s: 2, value: 0 },
			{ time_s: 6, value: 1 },
			{ time_s: 9, value: 0.1 }
		]);
	});
});
