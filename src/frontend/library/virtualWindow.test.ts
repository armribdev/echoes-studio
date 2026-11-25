import { describe, expect, it } from 'vitest';
import { gridColumns, scrollTopToReveal, virtualWindow } from './virtualWindow';

describe('virtualWindow', () => {
	const base = { scrollTop: 0, viewportHeight: 240, itemHeight: 24, perRow: 1, overscanRows: 10 };

	it('mounts the viewport plus the overscan of a list', () => {
		const win = virtualWindow({ ...base, total: 1000 });
		expect(win).toMatchObject({ firstRow: 0, lastRow: 20, start: 0, end: 20, totalRows: 1000 });
	});

	it('follows the scroll position and never goes past the ends', () => {
		const mid = virtualWindow({ ...base, scrollTop: 2400, total: 1000 });
		expect(mid).toMatchObject({ firstRow: 90, lastRow: 120 });
		const past = virtualWindow({ ...base, scrollTop: 100000, total: 50 });
		expect(past.lastRow).toBe(50);
		expect(past.end).toBe(50);
	});

	it('mounts whole rows of a grid and clips the last partial one', () => {
		const win = virtualWindow({ ...base, itemHeight: 68, perRow: 3, total: 10, overscanRows: 1 });
		expect(win.totalRows).toBe(4);
		expect(win).toMatchObject({ start: 0, end: 10 });
		const scrolled = virtualWindow({
			...base,
			itemHeight: 68,
			perRow: 3,
			total: 100,
			scrollTop: 680,
			overscanRows: 0
		});
		expect(scrolled).toMatchObject({ firstRow: 10, start: 30 });
	});

	it('handles an empty result list', () => {
		expect(virtualWindow({ ...base, total: 0 })).toMatchObject({ totalRows: 0, start: 0, end: 0 });
	});
});

describe('gridColumns', () => {
	it('fits as many tiles as the width allows, at least one', () => {
		expect(gridColumns(340, 100, 4)).toBe(3);
		expect(gridColumns(212, 100, 4)).toBe(2);
		expect(gridColumns(60, 100, 4)).toBe(1);
		expect(gridColumns(0, 100, 4)).toBe(1);
	});
});

describe('scrollTopToReveal', () => {
	const base = { perRow: 3, itemHeight: 68, padding: 4, viewportHeight: 200 };

	it('keeps the scroll when the item is already visible', () => {
		expect(scrollTopToReveal({ ...base, index: 4, scrollTop: 0 })).toBe(0);
	});

	it('scrolls up to the row of an item above the viewport', () => {
		expect(scrollTopToReveal({ ...base, index: 0, scrollTop: 300 })).toBe(4);
	});

	it('scrolls down just enough for an item below the viewport', () => {
		const top = 4 + 5 * 68;
		expect(scrollTopToReveal({ ...base, index: 15, scrollTop: 0 })).toBe(top + 68 - 200);
	});
});
