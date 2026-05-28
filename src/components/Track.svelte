<!-- Track.svelte -->
<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import type { Clip as ClipType } from '../types/global';
	import Clip from './Clip.svelte';

	export let clips: ClipType[];
	export let scale: number;
	export let trackIndex: number;

	const dispatch = createEventDispatcher<{
		clipdragstart: { offsetPx: number; trackIndex: number; clipIndex: number };
		clipcontextmenu: { id: string; trackIndex: number; x: number; y: number };
	}>();
</script>

<div class="track">
	{#each clips as clip, clipIndex}
		<Clip
			id={clip.id}
			start={clip.start}
			end={clip.end}
			{scale}
			sample_rate={clip.sample_rate}
			peaks_64={clip.peaks_64}
			peaks_512={clip.peaks_512}
			peaks_4096={clip.peaks_4096}
			on:clipdragstart={(e) => dispatch('clipdragstart', { ...e.detail, trackIndex, clipIndex })}
			on:clipcontextmenu={(e) => dispatch('clipcontextmenu', { ...e.detail, trackIndex })}
		/>
	{/each}
</div>

<style lang="scss">
	.track {
		position: relative;
		height: 100px;
	}
</style>
