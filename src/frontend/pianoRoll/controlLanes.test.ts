import { describe, expect, it } from 'vitest';
import type { MidiControlPoint } from '../../types/global';
import {
	AFTERTOUCH,
	LANE_HEIGHT,
	MAX_LANE_HEIGHT,
	clampLaneHeight,
	formatLaneValue,
	PITCH_BEND,
	SUSTAIN,
	candidateLanes,
	lanePoints,
	lanesWithData,
	laneNeutral,
	rampPoints,
	smoothRampPoints,
	replaceLanePoints,
	replaceSpan,
	stepPath,
	strokePoints,
	valueToY,
	yToValue
} from './controlLanes';

function point(
	start: number,
	kind: MidiControlPoint['kind'],
	number: number,
	value: number
): MidiControlPoint {
	return { start, kind, number, value };
}

describe('lanesWithData', () => {
	it('lists presets first, then other controllers by number, then polyphonic pressures by key', () => {
		const lanes = lanesWithData([
			point(0, 'control_change', 74, 10),
			point(0, 'poly_pressure', 60, 40),
			point(0, 'control_change', 64, 127),
			point(0, 'pitch_bend', 0, 9000),
			point(1, 'control_change', 20, 5),
			point(1, 'control_change', 74, 12)
		]);
		expect(lanes.map((lane) => `${lane.kind}:${lane.number}`)).toEqual([
			'pitch_bend:0',
			'control_change:64',
			'control_change:20',
			'control_change:74',
			'poly_pressure:60'
		]);
	});

	it('is empty for a clip without controls', () => {
		expect(lanesWithData([])).toEqual([]);
	});
});

describe('clampLaneHeight', () => {
	it('keeps the height between the default and the maximum', () => {
		expect(clampLaneHeight(10)).toBe(LANE_HEIGHT);
		expect(clampLaneHeight(90.4)).toBe(90);
		expect(clampLaneHeight(1000)).toBe(MAX_LANE_HEIGHT);
	});
});

describe('candidateLanes', () => {
	const keys = (lanes: { kind: string; number: number }[]) =>
		lanes.map((lane) => `${lane.kind}:${lane.number}`);

	it('offers presets, then every drawable controller once, without the lanes already shown', () => {
		const candidates = candidateLanes([point(0, 'control_change', 74, 10)], [PITCH_BEND], []);
		const listed = keys(candidates);
		expect(listed.slice(0, 4)).toEqual([
			'control_change:1',
			'control_change:64',
			'control_change:11',
			'channel_pressure:0'
		]);
		expect(listed).not.toContain('pitch_bend:0');
		expect(listed).toContain('control_change:74');
		expect(listed).toContain('control_change:119');
		expect(listed).not.toContain('control_change:120');
		expect(new Set(listed).size).toBe(listed.length);
	});

	it('offers the polyphonic pressure of each played key once, after the controllers', () => {
		const listed = keys(candidateLanes([], [], [64, 60, 64]));
		expect(listed.slice(-2)).toEqual(['poly_pressure:60', 'poly_pressure:64']);
		expect(listed.filter((lane) => lane.startsWith('poly_pressure'))).toHaveLength(2);
	});

	it('hides a polyphonic lane that is already shown', () => {
		const shown = [{ kind: 'poly_pressure' as const, number: 60 }];
		expect(keys(candidateLanes([], shown, [60, 62]))).toEqual(
			expect.not.arrayContaining(['poly_pressure:60'])
		);
	});

	it('keeps a controller above the drawable range when the clip holds data for it', () => {
		expect(keys(candidateLanes([point(0, 'control_change', 123, 1)], [], []))).toContain(
			'control_change:123'
		);
	});
});

describe('replaceLanePoints', () => {
	const controls = [
		point(1, 'pitch_bend', 0, 9000),
		point(0, 'control_change', 64, 127),
		point(2, 'poly_pressure', 60, 40)
	];

	it('swaps one lane and keeps every other control', () => {
		const next = replaceLanePoints(controls, PITCH_BEND, [point(3, 'pitch_bend', 0, 8000)]);
		expect(lanePoints(next, PITCH_BEND)).toEqual([point(3, 'pitch_bend', 0, 8000)]);
		expect(lanePoints(next, SUSTAIN)).toHaveLength(1);
		expect(next.some((p) => p.kind === 'poly_pressure')).toBe(true);
	});

	it('orders the result by position', () => {
		const next = replaceLanePoints(controls, SUSTAIN, [point(5, 'control_change', 64, 0)]);
		expect(next.map((p) => p.start)).toEqual([1, 2, 5]);
	});

	it('swaps the pressure of one key only', () => {
		const key60 = { kind: 'poly_pressure' as const, number: 60 };
		const next = replaceLanePoints([...controls, point(3, 'poly_pressure', 62, 10)], key60, [
			point(4, 'poly_pressure', 60, 90)
		]);
		expect(lanePoints(next, key60)).toEqual([point(4, 'poly_pressure', 60, 90)]);
		expect(next).toContainEqual(point(3, 'poly_pressure', 62, 10));
	});

	it('clears a lane', () => {
		expect(lanePoints(replaceLanePoints(controls, PITCH_BEND, []), PITCH_BEND)).toEqual([]);
	});
});

describe('value and position', () => {
	it('maps the lane ends to the range ends and back', () => {
		expect(yToValue(valueToY(16383, PITCH_BEND, LANE_HEIGHT), PITCH_BEND, LANE_HEIGHT)).toBe(16383);
		expect(yToValue(valueToY(0, PITCH_BEND, LANE_HEIGHT), PITCH_BEND, LANE_HEIGHT)).toBe(0);
		expect(yToValue(valueToY(127, SUSTAIN, LANE_HEIGHT), SUSTAIN, LANE_HEIGHT)).toBe(127);
	});

	it('clamps a pointer outside the lane', () => {
		expect(yToValue(-40, SUSTAIN, LANE_HEIGHT)).toBe(127);
		expect(yToValue(500, SUSTAIN, LANE_HEIGHT)).toBe(0);
	});

	it('puts the bend centre in the middle of the lane', () => {
		expect(valueToY(8192, PITCH_BEND, LANE_HEIGHT)).toBeCloseTo(LANE_HEIGHT / 2, 0);
	});
});

describe('neutral values', () => {
	it('matches the engine', () => {
		expect(laneNeutral(PITCH_BEND)).toBe(8192);
		expect(laneNeutral(AFTERTOUCH)).toBe(0);
		expect(laneNeutral(SUSTAIN)).toBe(0);
		expect(laneNeutral({ kind: 'control_change', number: 11 })).toBe(127);
		expect(laneNeutral({ kind: 'control_change', number: 74 })).toBeNull();
	});

	it('gives a polyphonic pressure none, even on the key numbered like a neutral controller', () => {
		expect(laneNeutral({ kind: 'poly_pressure', number: 64 })).toBeNull();
	});
});

describe('stepPath', () => {
	it('holds each value until the next point and starts from neutral', () => {
		const path = stepPath(
			[
				{ beat: 1, value: 127 },
				{ beat: 2, value: 0 }
			],
			SUSTAIN,
			100,
			400,
			LANE_HEIGHT
		);
		expect(path.startsWith(`M 0 ${valueToY(0, SUSTAIN, LANE_HEIGHT)} H 100`)).toBe(true);
		expect(path).toContain(`H 200 V ${valueToY(0, SUSTAIN, LANE_HEIGHT)}`);
		expect(path.endsWith('H 400')).toBe(true);
	});

	it('is empty without points', () => {
		expect(stepPath([], SUSTAIN, 100, 400, LANE_HEIGHT)).toBe('');
	});
});

describe('ramp and stroke', () => {
	it('ramps in either direction with both ends exact', () => {
		const ramp = rampPoints({ beat: 2, value: 0 }, { beat: 0, value: 100 }, 0.5);
		expect(ramp[0]).toEqual({ beat: 0, value: 100 });
		expect(ramp[ramp.length - 1]).toEqual({ beat: 2, value: 0 });
		expect(ramp).toHaveLength(5);
		expect(ramp.map((p) => p.value)).toEqual([100, 75, 50, 25, 0]);
	});

	it('eases a smooth ramp at both ends while keeping them exact', () => {
		const ramp = smoothRampPoints({ beat: 0, value: 0 }, { beat: 4, value: 100 }, 0.5);
		expect(ramp[0]).toEqual({ beat: 0, value: 0 });
		expect(ramp[ramp.length - 1]).toEqual({ beat: 4, value: 100 });
		const straight = rampPoints({ beat: 0, value: 0 }, { beat: 4, value: 100 }, 0.5);
		expect(ramp[1].value).toBeLessThan(straight[1].value);
		expect(ramp[ramp.length - 2].value).toBeGreaterThan(straight[straight.length - 2].value);
		expect(ramp.map((p) => p.value)).toEqual([...ramp.map((p) => p.value)].sort((a, b) => a - b));
	});

	it('keeps a single point for an instant ramp', () => {
		expect(rampPoints({ beat: 1, value: 5 }, { beat: 1, value: 9 }, 0.25)).toEqual([
			{ beat: 1, value: 9 }
		]);
	});

	it('reduces a free-hand stroke to one value per step without repeats', () => {
		const stroke = strokePoints(
			[
				{ beat: 0.0, value: 10 },
				{ beat: 0.01, value: 20 },
				{ beat: 0.07, value: 20 },
				{ beat: 0.13, value: 20 },
				{ beat: 0.2, value: 30 }
			],
			0.0625
		);
		expect(stroke).toEqual([
			{ beat: 0, value: 20 },
			{ beat: 0.1875, value: 30 }
		]);
	});

	it('replaces what lay under the drawn span only', () => {
		const next = replaceSpan(
			[
				{ beat: 0, value: 1 },
				{ beat: 1, value: 2 },
				{ beat: 4, value: 3 }
			],
			[
				{ beat: 0.5, value: 9 },
				{ beat: 2, value: 8 }
			]
		);
		expect(next).toEqual([
			{ beat: 0, value: 1 },
			{ beat: 0.5, value: 9 },
			{ beat: 2, value: 8 },
			{ beat: 4, value: 3 }
		]);
	});
});

describe('formatLaneValue', () => {
	it('shows a bend as an offset from the centre', () => {
		expect(formatLaneValue(PITCH_BEND, 8192)).toBe('0');
		expect(formatLaneValue(PITCH_BEND, 10240)).toBe('+2048');
		expect(formatLaneValue(PITCH_BEND, 6144)).toBe('-2048');
		expect(formatLaneValue(SUSTAIN, 127)).toBe('127');
	});
});
