<script lang="ts">
	export let start: number;
	export let end: number;
	export let scale: number;
</script>

<div class="clip" style="left: {start * scale}px; width: {(end - start) * scale}px"></div>

<style lang="scss">
	.clip {
		position: absolute;
		display: block;
		background-color: pink;
		border-radius: 5px;
		top: 0;
		bottom: 0;
	}
</style>
