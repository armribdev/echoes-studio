<!-- VuMeter.svelte — two slim vertical bars (L, R) showing peak amplitude. -->
<script lang="ts">
	/** Peak amplitude [0, 1] for left channel. */
	export let peakL: number = 0;
	/** Peak amplitude [0, 1] for right channel. */
	export let peakR: number = 0;

	const DB_FLOOR = -60;
	const DB_CLIP = 0;

	function toDb(linear: number): number {
		if (linear <= 0) return DB_FLOOR;
		return Math.max(DB_FLOOR, 20 * Math.log10(linear));
	}

	/** Map dBFS → bar fill height in percent (0 = silent, 100 = 0 dBFS). */
	function dbToPercent(db: number): number {
		return Math.max(0, Math.min(100, ((db - DB_FLOOR) / (DB_CLIP - DB_FLOOR)) * 100));
	}

	$: heightL = dbToPercent(toDb(peakL));
	$: heightR = dbToPercent(toDb(peakR));

	/** Color based on level: green → yellow → red. */
	function barColor(pct: number): string {
		if (pct >= 95) return 'var(--meter-red,   #ff4444)';
		if (pct >= 75) return 'var(--meter-yellow, #ffd93d)';
		return 'var(--meter-green,  #6bcb77)';
	}

	$: colorL = barColor(heightL);
	$: colorR = barColor(heightR);
</script>

<div class="vu" aria-hidden="true">
	<div class="bar-wrap">
		<div class="bar" style="height: {heightL}%; background: {colorL};"></div>
	</div>
	<div class="bar-wrap">
		<div class="bar" style="height: {heightR}%; background: {colorR};"></div>
	</div>
</div>

<style lang="scss">
	.vu {
		display: flex;
		align-items: flex-end;
		gap: 1px;
		height: 44px;
		flex-shrink: 0;
	}

	.bar-wrap {
		width: 3px;
		height: 100%;
		background: rgba(255, 255, 255, 0.06);
		border-radius: 1px;
		overflow: hidden;
		display: flex;
		align-items: flex-end;
	}

	.bar {
		width: 100%;
		border-radius: 1px;
		transition:
			height 0.04s linear,
			background 0.08s ease;
		will-change: height;
	}
</style>
