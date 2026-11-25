<!-- Playhead.svelte — tête de lecture partagée -->
<script lang="ts">
	/** Position horizontale en px (translateX). */
	export let x = 0;
</script>

<div class="playhead" style="transform: translateX({x}px)"></div>

<style lang="scss">
	@use '../variables.scss' as *;

	.playhead {
		position: absolute;
		top: 0;
		left: 0;
		width: 2px;
		height: 100%;
		background-color: $on-solid;
		pointer-events: none;
		z-index: 10;
		will-change: transform;

		&::before {
			content: '';
			position: absolute;
			left: -5px;
			width: 0;
			height: 0;
			border-left: 6px solid transparent;
			border-right: 6px solid transparent;
			border-top: 8px solid $on-solid;
		}
	}
</style>
