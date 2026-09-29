<!-- Track.svelte -->
<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import type { Clip as ClipType, MidiClip as MidiClipType } from '../types/global';
	import Clip from './Clip.svelte';
	import MidiClip from './MidiClip.svelte';
	import { pendingImports } from '../frontend/audio/pendingImports';
	import { buildClipDisplayItems } from '../frontend/audio/clipDisplay';
	import { midiRecordingPreview } from '../stores/midiRecording';
	import { transport } from '../stores/transport';
	import { buildPreviewClip } from '../frontend/midiRecordingPreview';

	export let trackId: string;
	export let clips: ClipType[];
	export let midiClips: MidiClipType[];
	export let scale: number;
	export let trackIndex: number;
	export let trackColor: string = '#4d96ff';
	export let bpm: number = 120;

	const dispatch = createEventDispatcher<{
		clipdragstart: {
			offsetPx: number;
			trackIndex: number;
			clipId: string;
			pendingImportId?: string;
		};
		clipresizestart: {
			edge: 'left' | 'right';
			clientX: number;
			trackIndex: number;
			clipId: string;
		};
		clipcontextmenu: { id: string; trackIndex: number; x: number; y: number };
		pendingpainted: { pendingImportId: string };
		midiclip_dragstart: { offsetPx: number; trackIndex: number; clipId: string; duplicate: boolean };
		midiclip_resizestart: { edge: 'left' | 'right'; clientX: number; trackIndex: number; clipId: string };
		midiclip_repeatstart: { clientX: number; trackIndex: number; clipId: string };
		midiclip_contextmenu: { id: string; trackIndex: number; x: number; y: number };
		midiclip_dblclick: { id: string; trackIndex: number };
		midiclip_activate: { id: string; trackIndex: number };
		trackdblclick: { offsetPx: number; trackIndex: number };
	}>();

	$: displayClips = buildClipDisplayItems(clips, $pendingImports, trackId);
	$: recordingClip =
		$midiRecordingPreview?.trackId === trackId
			? buildPreviewClip($midiRecordingPreview, $transport.position, bpm)
			: null;

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
	{#each displayClips as item (item.key)}
		<Clip
			id={item.id}
			start={item.start}
			end={item.end}
			offset={item.offset}
			{scale}
			sample_rate={item.sample_rate}
			peaks_64={item.peaks_64}
			peaks_512={item.peaks_512}
			peaks_4096={item.peaks_4096}
			pending={item.pending}
			loadProgress={item.loadProgress}
			pendingImportId={item.pendingImportId ?? null}
			on:clipdragstart={(e) =>
				dispatch('clipdragstart', {
					...e.detail,
					trackIndex,
					clipId: item.id,
					pendingImportId: item.pendingImportId,
				})}
			on:clipresizestart={(e) =>
				dispatch('clipresizestart', { ...e.detail, trackIndex, clipId: item.id })}
			on:clipcontextmenu={(e) => dispatch('clipcontextmenu', { ...e.detail, trackIndex })}
			on:pendingpainted={(e) => dispatch('pendingpainted', e.detail)}
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
	{#if recordingClip}
		<MidiClip
			id="midi-recording"
			start={recordingClip.start}
			end={recordingClip.end}
			offset={0}
			{scale}
			{bpm}
			notes={recordingClip.notes}
			color={trackColor}
			recording
		/>
	{/if}
</div>

<style lang="scss">
	.track {
		position: relative;
		height: 100px;
	}
</style>
