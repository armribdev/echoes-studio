onmessage = function (event) {
	const { raw_samples, sample_rate, scale } = event.data;

	const offScreenCanvas = new OffscreenCanvas(100, 100);
	const ctx = offScreenCanvas.getContext('2d');
	if (!ctx) return;

	drawWaveform(ctx, raw_samples, sample_rate, scale);

	postMessage({
		imageData: ctx.getImageData(0, 0, offScreenCanvas.width, offScreenCanvas.height).data
	});
};

function drawWaveform(
	context: OffscreenCanvasRenderingContext2D,
	raw_samples: number[],
	sample_rate: number,
	scale: number
) {
	if (!context) {
		return;
	}
	context.clearRect(0, 0, context.canvas.width, context.canvas.height);
	context.strokeStyle = 'red';
	context.lineWidth = 1;

	context.beginPath();

	const startX = 0;
	const startY = (0.5 - raw_samples[0] / 2) * context.canvas.height;
	context.moveTo(startX, startY);

	for (let i = 1; i < raw_samples.length; i++) {
		const x = (i * scale) / sample_rate;
		const y = (0.5 - raw_samples[i] / 2) * context.canvas.height;
		context.lineTo(x, y);
	}

	context.stroke();
}
