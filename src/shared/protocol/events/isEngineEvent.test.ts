import { describe, expect, it } from 'vitest';
import { isEngineEvent } from './index';

describe('isEngineEvent', () => {
	it('accepts envelopes with type and payload', () => {
		expect(isEngineEvent({ type: 'TRACK_CREATED', payload: { id: 't1' } })).toBe(true);
	});

	it('rejects non-objects and incomplete envelopes', () => {
		expect(isEngineEvent(null)).toBe(false);
		expect(isEngineEvent('TRACK_CREATED')).toBe(false);
		expect(isEngineEvent({ type: 'TRACK_CREATED' })).toBe(false);
		expect(isEngineEvent({ payload: {} })).toBe(false);
	});
});
