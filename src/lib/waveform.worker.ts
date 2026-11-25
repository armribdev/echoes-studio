type Peaks = [number, number][];

interface RenderMessage {
	peaks_64: Peaks;
	peaks_512: Peaks;
	peaks_4096: Peaks;
	sample_rate: number;
	scale: number;
	width: number;
	height: number;
	/** Cache key echoed back to the caller (encodes zoom bucket + trim offset). */
	key: string;
	/** Seconds skipped at the clip's left edge (left-trim). */
	offset: number;
	/** CSS color of the wave (the worker cannot read the page stylesheet). */
	fill: string;
}

onmessage = function (event: MessageEvent<RenderMessage>) {
	const { peaks_64, peaks_512, peaks_4096, sample_rate, scale, width, height, key, offset, fill } =
		event.data;

	const canvas = new OffscreenCanvas(width, height);
	const ctx = canvas.getContext('2d')!;

	drawWaveform(ctx, peaks_64, peaks_512, peaks_4096, sample_rate, scale, offset, fill);

	const bitmap = canvas.transferToImageBitmap();
	postMessage({ bitmap, key }, { transfer: [bitmap] });
};

function selectPeaks(
	peaks_64: Peaks,
	peaks_512: Peaks,
	peaks_4096: Peaks,
	sample_rate: number,
	scale: number
): { peaks: Peaks; chunkSize: number } {
	const samplesPerPixel = sample_rate / scale;
	// Prefer the lane matching zoom, but fall back to whatever we have
	// (progressive import only streams peaks_4096).
	if (samplesPerPixel < 512 && peaks_64.length > 0) return { peaks: peaks_64, chunkSize: 64 };
	if (samplesPerPixel < 4096 && peaks_512.length > 0) return { peaks: peaks_512, chunkSize: 512 };
	if (peaks_4096.length > 0) return { peaks: peaks_4096, chunkSize: 4096 };
	if (peaks_512.length > 0) return { peaks: peaks_512, chunkSize: 512 };
	return { peaks: peaks_64, chunkSize: 64 };
}

function drawWaveform(
	ctx: OffscreenCanvasRenderingContext2D,
	peaks_64: Peaks,
	peaks_512: Peaks,
	peaks_4096: Peaks,
	sample_rate: number,
	scale: number,
	offset: number,
	fill: string
) {
	const W = ctx.canvas.width;
	const H = ctx.canvas.height;

	ctx.clearRect(0, 0, W, H);
	ctx.fillStyle = fill;

	const { peaks, chunkSize } = selectPeaks(peaks_64, peaks_512, peaks_4096, sample_rate, scale);
	const secPerChunk = chunkSize / sample_rate;

	for (let px = 0; px < W; px++) {
		const timeStart = offset + px / scale;
		const timeEnd = offset + (px + 1) / scale;

		const chunkStart = Math.floor(timeStart / secPerChunk);
		const chunkEnd = Math.floor(timeEnd / secPerChunk);

		// Always include at least one chunk even when zoomed in past peak resolution
		const limit = Math.min(Math.max(chunkStart + 1, chunkEnd + 1), peaks.length);

		let pMin = 0;
		let pMax = 0;
		for (let c = chunkStart; c < limit; c++) {
			const [lo, hi] = peaks[c];
			if (lo < pMin) pMin = lo;
			if (hi > pMax) pMax = hi;
		}

		const yTop = (0.5 - pMax * 0.5) * H;
		const yBot = (0.5 - pMin * 0.5) * H;
		ctx.fillRect(px, yTop, 1, Math.max(1, yBot - yTop));
	}
}
