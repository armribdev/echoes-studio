<script>
	export let value, min, max;
	export let rotRange = 2 * Math.PI * 0.83;
	export let pixelRange = 200;
	export let startRotation = -Math.PI * 0.83;

	let startY, startValue;
	$: valueRange = max - min;
	$: rotation = startRotation + ((value - min) / valueRange) * rotRange;

	function clamp(num, min, max) {
		return Math.max(min, Math.min(num, max));
	}

	function pointerMove({ clientY }) {
		const valueDiff = (valueRange * (clientY - startY)) / pixelRange;
		value = clamp(startValue - valueDiff, min, max);
	}

	function pointerDown({ clientY }) {
		console.log({ clientY });
		startY = clientY;
		startValue = value;
		window.addEventListener('pointermove', pointerMove);
		window.addEventListener('pointerup', pointerUp);
	}

	function pointerUp() {
		window.removeEventListener('pointermove', pointerMove);
		window.removeEventListener('pointerup', pointerUp);
	}
</script>

<div class="knob" style="--rotation: {rotation}" on:pointerdown={pointerDown}>
	<div class="mark"></div>
</div>

<style lang="scss">
	@import '../variables.scss';
	.knob {
		display: block;
		width: 20px;
		height: 20px;
		padding: 0;
		border-radius: 50%;
		background-color: $border-color;
		transform: rotate(calc(var(--rotation) * 1rad));
		transform-origin: 50% 50%;
		cursor: pointer;
		display: inline-block;
	}

	.mark {
		width: 2px;
		height: 6px;
		background-color: white;
		margin-left: 9px;
	}
</style>
