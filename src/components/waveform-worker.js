onmessage = function (e) {
	const { start, end, scale, raw_samples, sample_rate } = e.data;
	const points = [];

	for (let i = start; i < end; i++) {
		const x = (i * scale) / sample_rate;
		const y = 0.5 - raw_samples[i] / 2;
		points.push({ x, y });
	}

	postMessage(points);
};
