<!-- Track.svelte -->
<script lang="ts">
	import type { Clip as ClipType } from '../types/global';
	import Clip from './Clip.svelte';
	export let clips: ClipType[];
	export let scale: number;
</script>

<div class="track">
	{#each clips as { start, end }}
		<Clip {start} {end} {scale} />
	{/each}
</div>

<style lang="scss">
	.track {
		position: relative;
		height: 100px;
	}
</style>
