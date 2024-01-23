<!-- Clip.svelte -->
<script lang="ts">
	export let start: number;
	export let end: number;
	export let scale: number;
</script>

<div class="clip" style="left: {start * scale}px; width: {(end - start) * scale}px"></div>

<style lang="scss">
	.clip {
		position: absolute;
		height: 100%;
		background-color: pink;
		border-radius: 5px;
	}
</style>
