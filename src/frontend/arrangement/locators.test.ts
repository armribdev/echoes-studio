import { describe, expect, it } from 'vitest';
import {
	collectLocators,
	defaultRegionLengthBeats,
	defaultRegionSpanAt,
	navigateLocator
} from './locators';

describe('defaultRegionLengthBeats / defaultRegionSpanAt', () => {
	it('spans 4 bars from the bar containing the beat', () => {
		expect(defaultRegionLengthBeats(4)).toBe(16);
		expect(defaultRegionSpanAt(5, 4)).toEqual({ start: 4, end: 20 });
		expect(defaultRegionSpanAt(0, 3)).toEqual({ start: 0, end: 12 });
	});

	it('treats invalid numerator as 1', () => {
		expect(defaultRegionLengthBeats(0)).toBe(4);
		expect(defaultRegionSpanAt(2.5, 0)).toEqual({ start: 2, end: 6 });
	});
});

describe('collectLocators', () => {
	it('merges markers and regions sorted by beat then id', () => {
		const entries = collectLocators(
			[
				{ id: 'm2', name: 'B', position_beats: 8 },
				{ id: 'm1', name: 'A', position_beats: 0 }
			],
			[
				{ id: 'r1', name: 'Verse', start_beats: 8, end_beats: 16 },
				{ id: 'r0', name: 'Intro', start_beats: 0, end_beats: 8 }
			]
		);
		expect(entries.map((e) => `${e.kind}:${e.id}`)).toEqual([
			'marker:m1',
			'region:r0',
			'marker:m2',
			'region:r1'
		]);
	});
});

describe('navigateLocator', () => {
	const entries = collectLocators(
		[
			{ id: 'm1', name: 'A', position_beats: 0 },
			{ id: 'm2', name: 'B', position_beats: 16 }
		],
		[{ id: 'r1', name: 'Loop', start_beats: 8, end_beats: 12 }]
	);

	it('finds the next locator after the playhead and wraps', () => {
		expect(navigateLocator(entries, null, 4, 1)?.id).toBe('r1');
		expect(navigateLocator(entries, null, 20, 1)?.id).toBe('m1');
	});

	it('finds the previous locator before the playhead and wraps', () => {
		expect(navigateLocator(entries, null, 10, -1)?.id).toBe('r1');
		expect(navigateLocator(entries, null, 0, -1)?.id).toBe('m2');
	});

	it('returns null when there are no locators', () => {
		expect(navigateLocator([], null, 0, 1)).toBeNull();
	});
});
