<!-- ParamKnob.svelte — labelled knob with a live value readout, linear or logarithmic -->
<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import Knob from './Knob.svelte';

	export let label: string;
	export let value: number;
	export let min: number;
	export let max: number;
	export let defaultValue: number;
	/** `log` spreads the knob evenly across orders of magnitude (min must be > 0). */
	export let scale: 'linear' | 'log' = 'linear';
	export let format: (value: number) => string;
	export let color: string | undefined = undefined;
	/** Output only: follows the knob while it is being dragged. */
	export let live: number = value;

	const dispatch = createEventDispatcher<{ change: number; release: number }>();

	function toPosition(v: number): number {
		if (scale === 'log') return Math.log(v / min) / Math.log(max / min);
		return (v - min) / (max - min);
	}

	function fromPosition(position: number): number {
		if (scale === 'log') return min * (max / min) ** position;
		return min + position * (max - min);
	}

	let position = 0;
	$: position = toPosition(value);
	$: live = fromPosition(position);
</script>

<div class="param">
	<span class="label">{label}</span>
	<Knob
		min={0}
		max={1}
		defaultValue={toPosition(defaultValue)}
		bind:value={position}
		{color}
		on:change={(e) => dispatch('change', fromPosition(e.detail))}
		on:release={(e) => dispatch('release', fromPosition(e.detail))}
	/>
	<span class="value">{format(live)}</span>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.param {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 1px;
	}

	.label,
	.value {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.label {
		font-size: 7px;
		font-family: $font-mono;
		font-weight: 700;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: $text-faint;
		user-select: none;
	}

	.value {
		font-size: 8px;
		font-family: $font-mono;
		color: $text-subtle;
		white-space: nowrap;
	}
</style>
