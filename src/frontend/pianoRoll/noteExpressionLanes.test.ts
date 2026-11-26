import { describe, expect, it } from 'vitest';
import type { MidiNote } from '../../types/global';
import {
	NOTE_PITCH,
	NOTE_PRESSURE,
	NOTE_TIMBRE,
	candidateLanes,
	formatLaneValue,
	laneCentre,
	laneMax,
	laneNeutral,
	valueToY,
	yToValue
} from './controlLanes';
import {
	expressionLaneHasData,
	expressionLanePoints,
	expressionLanesWithData,
	noteRange,
	withExpressionPoints
} from './noteExpressionLanes';

const LANE_PX = 52;

function note(partial: Partial<MidiNote> = {}): MidiNote {
	return { id: 'n1', pitch: 60, start: 4, duration: 2, velocity: 100, ...partial };
}

describe('expressionLanePoints', () => {
	it('places the points in clip beats and scales them to the lane', () => {
		const withCurves = note({
			expressions: [
				{ start: 0, kind: 'pressure', value: 0 },
				{ start: 1, kind: 'pressure', value: 1 },
				{ start: 0.5, kind: 'timbre', value: 0.5 }
			]
		});
		expect(expressionLanePoints(withCurves, NOTE_PRESSURE)).toEqual([
			{ beat: 4, value: 0 },
			{ beat: 5, value: 127 }
		]);
		expect(expressionLanePoints(withCurves, NOTE_TIMBRE)).toEqual([{ beat: 4.5, value: 64 }]);
	});

	it('centres the micro-pitch lane on zero semitones', () => {
		const bent = note({
			expressions: [
				{ start: 0, kind: 'pitch', value: 0 },
				{ start: 1, kind: 'pitch', value: -24 },
				{ start: 1.5, kind: 'pitch', value: 24 }
			]
		});
		expect(expressionLanePoints(bent, NOTE_PITCH).map((p) => p.value)).toEqual([240, 0, 480]);
	});

	it('is empty for a note without curves', () => {
		expect(expressionLanePoints(note(), NOTE_PRESSURE)).toEqual([]);
	});
});

describe('withExpressionPoints', () => {
	it('stores the points relative to the note, sorted, and keeps the other kinds', () => {
		const original = note({ expressions: [{ start: 0.25, kind: 'timbre', value: 0.5 }] });
		const edited = withExpressionPoints(original, NOTE_PRESSURE, [
			{ beat: 5, value: 127 },
			{ beat: 4, value: 0 }
		]);
		expect(edited.expressions).toEqual([
			{ start: 0.25, kind: 'timbre', value: 0.5 },
			{ start: 0, kind: 'pressure', value: 0 },
			{ start: 1, kind: 'pressure', value: 1 }
		]);
	});

	it('round-trips through the lane without drift', () => {
		const original = note({
			expressions: [
				{ start: 0, kind: 'pitch', value: -3.5 },
				{ start: 1, kind: 'pitch', value: 12 }
			]
		});
		const again = withExpressionPoints(
			original,
			NOTE_PITCH,
			expressionLanePoints(original, NOTE_PITCH)
		);
		expect(again).toEqual(original);
	});

	it('drops points before the note or at and after its end', () => {
		const edited = withExpressionPoints(note(), NOTE_PRESSURE, [
			{ beat: 3.9, value: 10 },
			{ beat: 5.5, value: 20 },
			{ beat: 6, value: 30 },
			{ beat: 7, value: 40 }
		]);
		expect(edited.expressions).toEqual([{ start: 1.5, kind: 'pressure', value: 20 / 127 }]);
	});

	it('removes the field once the last curve is gone, as an old project has it', () => {
		const original = note({ expressions: [{ start: 0, kind: 'pressure', value: 0.5 }] });
		const cleared = withExpressionPoints(original, NOTE_PRESSURE, []);
		expect('expressions' in cleared).toBe(false);
		expect(original.expressions).toHaveLength(1);
	});
});

describe('expression lanes', () => {
	it('lists the kinds drawn on any note', () => {
		const notes = [
			note(),
			note({ id: 'n2', expressions: [{ start: 0, kind: 'timbre', value: 0.1 }] })
		];
		expect(expressionLanesWithData(notes)).toEqual([NOTE_TIMBRE]);
	});

	it('reports data only for the selected note', () => {
		const curved = note({ expressions: [{ start: 0, kind: 'pressure', value: 0.1 }] });
		expect(expressionLaneHasData(curved, NOTE_PRESSURE)).toBe(true);
		expect(expressionLaneHasData(curved, NOTE_TIMBRE)).toBe(false);
		expect(expressionLaneHasData(null, NOTE_PRESSURE)).toBe(false);
	});

	it('keeps points inside the note, a margin before its end', () => {
		const range = noteRange(note());
		expect(range.start).toBe(4);
		expect(range.end).toBeLessThan(6);
		expect(range.end).toBeGreaterThan(5.9);
	});

	it('are offered by the lane pickers', () => {
		const offered = candidateLanes([], [], []);
		expect(offered).toContainEqual(NOTE_PRESSURE);
		expect(offered).toContainEqual(NOTE_PITCH);
		expect(candidateLanes([], [NOTE_PITCH], [])).not.toContainEqual(NOTE_PITCH);
	});
});

describe('expression lane scale', () => {
	it('reads a pressure in 0..1 and a micro-pitch in semitones', () => {
		expect(formatLaneValue(NOTE_PRESSURE, 127)).toBe('1.00');
		expect(formatLaneValue(NOTE_TIMBRE, 0)).toBe('0.00');
		expect(formatLaneValue(NOTE_PITCH, 240)).toBe('0.0');
		expect(formatLaneValue(NOTE_PITCH, 255)).toBe('+1.5');
		expect(formatLaneValue(NOTE_PITCH, 225)).toBe('-1.5');
	});

	it('swings the micro-pitch around its centre and gives it no neutral level', () => {
		expect(laneCentre(NOTE_PITCH)).toBe(laneMax(NOTE_PITCH) / 2);
		expect(laneCentre(NOTE_PRESSURE)).toBeNull();
		expect(laneNeutral(NOTE_PITCH)).toBeNull();
	});

	it('maps the full lane height to the full range', () => {
		expect(yToValue(valueToY(480, NOTE_PITCH, LANE_PX), NOTE_PITCH, LANE_PX)).toBe(480);
		expect(yToValue(valueToY(127, NOTE_PRESSURE, LANE_PX), NOTE_PRESSURE, LANE_PX)).toBe(127);
	});
});
