import { describe, expect, it } from 'vitest';
import { computeRulerTicks, defaultArrangeMaxBars } from './ticks';

describe('computeRulerTicks', () => {
	it('emits bar labels and beat ticks at high zoom', () => {
		const ticks = computeRulerTicks({
			pxPerBeat: 40,
			numerator: 4,
			maxBars: 3
		});
		expect(ticks.major.map((t) => t.label)).toEqual(['1', '2', '3']);
		expect(ticks.beat.map((t) => t.label)).toEqual([
			'1.2',
			'1.3',
			'1.4',
			'2.2',
			'2.3',
			'2.4',
			'3.2',
			'3.3',
			'3.4'
		]);
	});

	it('thins bar labels when zoomed out', () => {
		const ticks = computeRulerTicks({
			pxPerBeat: 4,
			numerator: 4,
			maxBars: 8
		});
		expect(ticks.major.length).toBeLessThan(8);
		expect(ticks.beat).toHaveLength(0);
	});

	it('clips to the visible viewport', () => {
		const ticks = computeRulerTicks({
			pxPerBeat: 20,
			numerator: 4,
			scrollPx: 160,
			viewportWidthPx: 80,
			maxBars: 20
		});
		expect(ticks.major.every((t) => t.x >= 120 && t.x <= 280)).toBe(true);
	});
});

describe('defaultArrangeMaxBars', () => {
	it('covers about 30 minutes and clamps to 2000', () => {
		expect(defaultArrangeMaxBars(120, 4)).toBe(904);
		expect(defaultArrangeMaxBars(300, 4)).toBe(2000);
	});
});
