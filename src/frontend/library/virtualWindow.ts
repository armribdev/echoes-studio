export interface VirtualWindowInput {
	scrollTop: number;
	viewportHeight: number;
	itemHeight: number;
	/** Items per row: 1 for a list, the column count for a grid. */
	perRow: number;
	total: number;
	overscanRows: number;
}

export interface VirtualWindow {
	firstRow: number;
	lastRow: number;
	totalRows: number;
	/** Index range of the items to mount: `items.slice(start, end)`. */
	start: number;
	end: number;
}

export function virtualWindow(input: VirtualWindowInput): VirtualWindow {
	const { scrollTop, viewportHeight, itemHeight, perRow, total, overscanRows } = input;
	const totalRows = Math.ceil(total / perRow);
	const firstRow = Math.max(0, Math.floor(scrollTop / itemHeight) - overscanRows);
	const lastRow = Math.min(
		totalRows,
		Math.ceil((scrollTop + viewportHeight) / itemHeight) + overscanRows
	);
	return {
		firstRow,
		lastRow,
		totalRows,
		start: firstRow * perRow,
		end: Math.min(total, lastRow * perRow)
	};
}

export function gridColumns(width: number, minTileWidth: number, gap: number): number {
	return Math.max(1, Math.floor((width + gap) / (minTileWidth + gap)));
}

/** New scroll offset that brings the item into view, or `scrollTop` when it already is. */
export function scrollTopToReveal(input: {
	index: number;
	perRow: number;
	itemHeight: number;
	padding: number;
	scrollTop: number;
	viewportHeight: number;
}): number {
	const { index, perRow, itemHeight, padding, scrollTop, viewportHeight } = input;
	const rowTop = padding + Math.floor(index / perRow) * itemHeight;
	const rowBottom = rowTop + itemHeight;
	if (rowTop < scrollTop) return rowTop;
	if (rowBottom > scrollTop + viewportHeight) return rowBottom - viewportHeight;
	return scrollTop;
}
