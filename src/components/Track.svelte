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
	import { getPreviewPitchRange } from '../frontend/pianoRoll/previewLayout';
	import { t } from 'svelte-i18n';

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
		midiclip_dragstart: {
			offsetPx: number;
			trackIndex: number;
			clipId: string;
			duplicate: boolean;
		};
		midiclip_resizestart: {
			edge: 'left' | 'right';
			clientX: number;
			trackIndex: number;
			clipId: string;
		};
		midiclip_repeatstart: { clientX: number; trackIndex: number; clipId: string };
		midiclip_contextmenu: { id: string; trackIndex: number; x: number; y: number };
		midiclip_dblclick: { id: string; trackIndex: number };
		midiclip_activate: { id: string; trackIndex: number };
		trackdblclick: { offsetPx: number; trackIndex: number };
		insertsound: { trackIndex: number; clientX: number; clientY: number };
	}>();

	$: displayClips = buildClipDisplayItems(clips, $pendingImports, trackId);
	$: recordingClip =
		$midiRecordingPreview?.trackId === trackId
			? buildPreviewClip($midiRecordingPreview, $transport.position, bpm)
			: null;
	$: laneEmpty = displayClips.length === 0 && midiClips.length === 0 && !recordingClip;
	$: lanePitchRange = getPreviewPitchRange([
		...midiClips.flatMap((c) => c.notes),
		...(recordingClip?.notes ?? [])
	]);

	let hovered = false;

	function onTrackDblClick(e: MouseEvent) {
		const target = e.target;
		if (!(target instanceof HTMLElement)) return;
		if (target.closest('.clip, .midi-clip, .lane-insert')) return;

		e.stopPropagation();
		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		dispatch('trackdblclick', { offsetPx: e.clientX - rect.left, trackIndex });
	}

	function onInsertClick(e: MouseEvent) {
		e.stopPropagation();
		dispatch('insertsound', {
			trackIndex,
			clientX: e.clientX,
			clientY: e.clientY
		});
	}
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	class="track"
	class:track--empty={laneEmpty}
	on:dblclick={onTrackDblClick}
	on:pointerenter={() => (hovered = true)}
	on:pointerleave={() => (hovered = false)}
>
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
					pendingImportId: item.pendingImportId
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
			minPitch={lanePitchRange.min}
			maxPitch={lanePitchRange.max}
			on:midiclip_dragstart={(e) =>
				dispatch('midiclip_dragstart', { ...e.detail, trackIndex, clipId: clip.id })}
			on:midiclip_resizestart={(e) =>
				dispatch('midiclip_resizestart', { ...e.detail, trackIndex, clipId: clip.id })}
			on:midiclip_repeatstart={(e) =>
				dispatch('midiclip_repeatstart', { ...e.detail, trackIndex, clipId: clip.id })}
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
			minPitch={lanePitchRange.min}
			maxPitch={lanePitchRange.max}
			recording
		/>
	{/if}

	{#if laneEmpty || hovered}
		<button
			type="button"
			class="lane-insert"
			class:lane-insert--empty={laneEmpty}
			title={$t('mainPanel.insertSound')}
			aria-label={$t('mainPanel.insertSound')}
			on:click={onInsertClick}
		>
			+
		</button>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.track {
		position: relative;
		height: 100px;
	}

	.lane-insert {
		position: absolute;
		left: 8px;
		top: 50%;
		transform: translateY(-50%);
		z-index: 5;
		width: 28px;
		height: 28px;
		padding: 0;
		border: 1px dashed rgba(255, 255, 255, 0.22);
		border-radius: $radius-md;
		background: rgba(26, 26, 31, 0.72);
		color: rgba(247, 247, 249, 0.55);
		font-size: 18px;
		font-weight: 400;
		line-height: 1;
		cursor: pointer;
		opacity: 0;
		transition:
			opacity 0.12s,
			color 0.12s,
			border-color 0.12s,
			background 0.12s;

		&--empty {
			opacity: 0.55;
		}

		.track:hover & {
			opacity: 1;
		}

		&:hover {
			color: $text-color;
			border-color: rgba($accent-color, 0.65);
			border-style: solid;
			background: rgba($accent-color, 0.16);
		}
	}
</style>
