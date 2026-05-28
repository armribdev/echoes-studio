<!-- Track.svelte -->
<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import type { Clip as ClipType, MidiClip as MidiClipType } from '../types/global';
	import Clip from './Clip.svelte';
	import MidiClip from './MidiClip.svelte';

	export let clips: ClipType[];
	export let midiClips: MidiClipType[];
	export let scale: number;
	export let trackIndex: number;
	export let trackColor: string = '#4d96ff';
	export let bpm: number = 120;

	const dispatch = createEventDispatcher<{
		clipdragstart: { offsetPx: number; trackIndex: number; clipIndex: number };
		clipcontextmenu: { id: string; trackIndex: number; x: number; y: number };
		midiclip_dragstart: { offsetPx: number; trackIndex: number; clipId: string };
		midiclip_contextmenu: { id: string; trackIndex: number; x: number; y: number };
		midiclip_dblclick: { id: string; trackIndex: number };
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
	{#each midiClips as clip}
		<MidiClip
			id={clip.id}
			start={clip.start}
			end={clip.end}
			{scale}
			{bpm}
			notes={clip.notes}
			color={trackColor}
			on:midiclip_dragstart={(e) => dispatch('midiclip_dragstart', { ...e.detail, trackIndex, clipId: clip.id })}
			on:midiclip_contextmenu={(e) => dispatch('midiclip_contextmenu', { ...e.detail, trackIndex })}
			on:midiclip_dblclick={(e) => dispatch('midiclip_dblclick', { ...e.detail, trackIndex })}
		/>
	{/each}
</div>

<style lang="scss">
	.track {
		position: relative;
		height: 100px;
	}
</style>
