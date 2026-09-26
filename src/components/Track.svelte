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
		clipresizestart: { edge: 'left' | 'right'; clientX: number; trackIndex: number; clipIndex: number };
		clipcontextmenu: { id: string; trackIndex: number; x: number; y: number };
		midiclip_dragstart: { offsetPx: number; trackIndex: number; clipId: string; duplicate: boolean };
		midiclip_resizestart: { edge: 'left' | 'right'; clientX: number; trackIndex: number; clipId: string };
		midiclip_repeatstart: { clientX: number; trackIndex: number; clipId: string };
		midiclip_contextmenu: { id: string; trackIndex: number; x: number; y: number };
		midiclip_dblclick: { id: string; trackIndex: number };
		midiclip_activate: { id: string; trackIndex: number };
		trackdblclick: { offsetPx: number; trackIndex: number };
	}>();

	function onTrackDblClick(e: MouseEvent) {
		const target = e.target;
		if (!(target instanceof HTMLElement)) return;
		if (target.closest('.clip, .midi-clip')) return;

		e.stopPropagation();
		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		dispatch('trackdblclick', { offsetPx: e.clientX - rect.left, trackIndex });
	}
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div class="track" on:dblclick={onTrackDblClick}>
	{#each clips as clip, clipIndex}
		<Clip
			id={clip.id}
			start={clip.start}
			end={clip.end}
			offset={clip.offset}
			{scale}
			sample_rate={clip.sample_rate}
			peaks_64={clip.peaks_64}
			peaks_512={clip.peaks_512}
			peaks_4096={clip.peaks_4096}
			on:clipdragstart={(e) => dispatch('clipdragstart', { ...e.detail, trackIndex, clipIndex })}
			on:clipresizestart={(e) => dispatch('clipresizestart', { ...e.detail, trackIndex, clipIndex })}
			on:clipcontextmenu={(e) => dispatch('clipcontextmenu', { ...e.detail, trackIndex })}
		/>
	{/each}
	{#each midiClips as clip}
		<MidiClip
			id={clip.id}
			start={clip.start}
			end={clip.end}
			offset={clip.offset}
			{scale}
			{bpm}
			notes={clip.notes}
			color={trackColor}
			on:midiclip_dragstart={(e) => dispatch('midiclip_dragstart', { ...e.detail, trackIndex, clipId: clip.id })}
			on:midiclip_resizestart={(e) => dispatch('midiclip_resizestart', { ...e.detail, trackIndex, clipId: clip.id })}
			on:midiclip_repeatstart={(e) => dispatch('midiclip_repeatstart', { ...e.detail, trackIndex, clipId: clip.id })}
			on:midiclip_contextmenu={(e) => dispatch('midiclip_contextmenu', { ...e.detail, trackIndex })}
			on:midiclip_dblclick={(e) => dispatch('midiclip_dblclick', { ...e.detail, trackIndex })}
			on:midiclip_activate={(e) => dispatch('midiclip_activate', { ...e.detail, trackIndex })}
		/>
	{/each}
</div>

<style lang="scss">
	.track {
		position: relative;
		height: 100px;
	}
</style>
