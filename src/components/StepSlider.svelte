<script lang="ts">
	type OptionValue = string | number;

	export let value: OptionValue;
	/** All selectable steps — each with a display label. */
	export let options: Array<{ value: OptionValue; label: string }>;

	// Keep the internal index in sync with the bound value.
	$: idx = Math.max(
		0,
		options.findIndex((o) => o.value === value)
	);

	// Track-fill percentage (0 → 100) for the CSS gradient.
	$: fillPct = options.length <= 1 ? 0 : (idx / (options.length - 1)) * 100;

	function onInput(e: Event) {
		const i = +(e.target as HTMLInputElement).value;
		if (i >= 0 && i < options.length) value = options[i].value;
	}

	function selectByIndex(i: number) {
		if (i >= 0 && i < options.length) value = options[i].value;
	}

	// Center of label i must align with center of tick i.
	// Ticks live in a flex container with padding: 0 7px (= half-thumb width).
	// Tick i center = 7px + i/(N-1) * (containerWidth − 14px)
	//               = calc(pct% + (7 − pct*0.14)px)   where pct = i/(N-1)*100
	function labelStyle(i: number): string {
		const n = options.length;
		if (n <= 1) return 'left:50%;transform:translateX(-50%)';
		const pct = (i / (n - 1)) * 100;
		const offsetPx = 7 - (i / (n - 1)) * 14;
		return `left:calc(${pct}% + ${offsetPx}px);transform:translateX(-50%)`;
	}
</script>

<div class="step-slider">
	<!-- ── Range input (invisible thumb, custom styled track) ── -->
	<div class="track-wrap">
		<input
			class="range"
			type="range"
			min="0"
			max={options.length - 1}
			step="1"
			value={idx}
			on:input={onInput}
			style="--fill: {fillPct}%"
		/>
		<!-- Tick marks aligned to each step position -->
		<div class="ticks" aria-hidden="true">
			{#each options as _, i}
				<div class="tick" class:active={i === idx}></div>
			{/each}
		</div>
	</div>

	<!-- ── Step labels (also clickable) ── -->
	<div class="labels">
		{#each options as opt, i}
			<!-- svelte-ignore a11y-click-events-have-key-events -->
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<span
				class="label"
				class:active={i === idx}
				style={labelStyle(i)}
				on:click={() => selectByIndex(i)}>{opt.label}</span
			>
		{/each}
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.step-slider {
		display: flex;
		flex-direction: column;
		gap: 7px;
		user-select: none;
	}

	// ── Track wrapper ─────────────────────────────────────────────────────────
	.track-wrap {
		position: relative;
		height: 22px; // enough for thumb + ticks
		display: flex;
		align-items: center;
	}

	// ── Range input ───────────────────────────────────────────────────────────
	.range {
		@include range-slider;
		position: absolute;
		inset: 0;
		z-index: 2;
	}

	// ── Tick marks ────────────────────────────────────────────────────────────
	.ticks {
		position: absolute;
		inset: 0;
		display: flex;
		justify-content: space-between;
		align-items: flex-end;
		padding: 0 7px; // match half-thumb width so ticks align with thumb centres
		pointer-events: none;
		z-index: 1;
	}

	.tick {
		width: 1px;
		height: 5px;
		background: $surface-active;
		border-radius: 1px;
		transition:
			background 0.15s,
			height 0.15s;

		&.active {
			background: $accent-color;
			height: 7px;
		}
	}

	// ── Step labels ───────────────────────────────────────────────────────────
	.labels {
		position: relative;
		height: 14px;
	}

	.label {
		position: absolute;
		top: 0;
		font-size: 10px;
		font-family: $font-mono;
		color: $text-faint;
		cursor: pointer;
		line-height: 1;
		transition: color $duration-base;
		white-space: nowrap;

		&:hover {
			color: $text-secondary;
		}

		&.active {
			color: $accent-color;
			font-weight: 600;
		}
	}
</style>
