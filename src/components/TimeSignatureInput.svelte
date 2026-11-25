<script lang="ts">
	import CustomSelect from './CustomSelect.svelte';
	import NumberInput from './NumberInput.svelte';

	export let numerator: number;
	export let denominator: number;

	const denominators = [2, 4, 8, 16].map((value) => ({ value, label: String(value) }));
</script>

<div class="time-signature">
	<NumberInput bind:value={numerator} min={1} max={16} width="42px" />
	<span class="slash">/</span>
	<CustomSelect small bind:value={denominator} options={denominators} />
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.time-signature {
		display: flex;
		align-items: center;
		gap: 7px;
	}

	.slash {
		font-size: 15px;
		color: $text-subtle;
	}
</style>
