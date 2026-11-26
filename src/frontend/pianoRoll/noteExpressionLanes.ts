import type { MidiNote, NoteExpressionKind, NoteExpressionPoint } from '../../types/global';
import {
	EXPRESSION_LANES,
	isExpressionLane,
	PITCH_EXPRESSION_STEPS_PER_SEMITONE,
	type ControlLane,
	type ExpressionLane,
	type LanePoint
} from './controlLanes';

/** Smallest distance between a point and the end of its note: the engine drops a point at the end. */
export const EXPRESSION_END_MARGIN_BEATS = 1 / 128;

const SEVEN_BIT_MAX = 127;
const PITCH_CENTRE_STEPS = 24 * PITCH_EXPRESSION_STEPS_PER_SEMITONE;

/** The span of the note a lane edits, in clip beats; points cannot leave it. */
export interface LaneRange {
	start: number;
	end: number;
}

const KIND_OF_LANE: Record<ExpressionLane['kind'], NoteExpressionKind> = {
	note_pressure: 'pressure',
	note_timbre: 'timbre',
	note_pitch: 'pitch'
};

function toLaneValue(kind: NoteExpressionKind, value: number): number {
	if (kind === 'pitch')
		return Math.round(value * PITCH_EXPRESSION_STEPS_PER_SEMITONE + PITCH_CENTRE_STEPS);
	return Math.round(value * SEVEN_BIT_MAX);
}

function fromLaneValue(kind: NoteExpressionKind, value: number): number {
	if (kind === 'pitch') return (value - PITCH_CENTRE_STEPS) / PITCH_EXPRESSION_STEPS_PER_SEMITONE;
	return value / SEVEN_BIT_MAX;
}

export function expressionKindOf(lane: ExpressionLane): NoteExpressionKind {
	return KIND_OF_LANE[lane.kind];
}

export function noteRange(note: MidiNote): LaneRange {
	return { start: note.start, end: note.start + note.duration - EXPRESSION_END_MARGIN_BEATS };
}

/** The curve of one kind as lane points, in clip beats (the note stores them relative to itself). */
export function expressionLanePoints(note: MidiNote, lane: ExpressionLane): LanePoint[] {
	const kind = expressionKindOf(lane);
	return (note.expressions ?? [])
		.filter((point) => point.kind === kind)
		.map((point) => ({ beat: note.start + point.start, value: toLaneValue(kind, point.value) }));
}

/** The note with the curve of one kind replaced; points outside the note are dropped. */
export function withExpressionPoints(
	note: MidiNote,
	lane: ExpressionLane,
	points: LanePoint[]
): MidiNote {
	const kind = expressionKindOf(lane);
	const edited: NoteExpressionPoint[] = points
		.map((point) => ({
			start: point.beat - note.start,
			kind,
			value: fromLaneValue(kind, point.value)
		}))
		.filter((point) => point.start >= 0 && point.start < note.duration)
		.sort((a, b) => a.start - b.start);
	const kept = (note.expressions ?? []).filter((point) => point.kind !== kind);
	const expressions = [...kept, ...edited];
	const next: MidiNote = { ...note, expressions };
	if (expressions.length === 0) delete next.expressions;
	return next;
}

/** The expression lanes whose curve exists on at least one note. */
export function expressionLanesWithData(notes: MidiNote[]): ExpressionLane[] {
	const present = new Set(notes.flatMap((note) => (note.expressions ?? []).map((p) => p.kind)));
	return EXPRESSION_LANES.filter((lane) => present.has(expressionKindOf(lane)));
}

export function expressionLaneHasData(note: MidiNote | null, lane: ControlLane): boolean {
	return note !== null && isExpressionLane(lane) && expressionLanePoints(note, lane).length > 0;
}
