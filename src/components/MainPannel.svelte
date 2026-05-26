<script lang="ts">
	import Knob from './Knob.svelte';
	import Track from './Track.svelte';
	import type { Track as TrackType, Clip } from '../types/global';
	import { invoke } from '@tauri-apps/api/core';
	import { open } from '@tauri-apps/plugin-dialog';
	import { listen } from '@tauri-apps/api/event';
	import { onMount } from 'svelte';
	import { transport } from '../stores/transport';
	import { selection } from '../stores/selection';
	import { tempo } from '../stores/tempo';

	let tracks: TrackType[] = [
		{ name: 'Piano', volume: 60, clips: [] },
		{ name: 'Guitare', volume: 80, clips: [] }
	];

	let logScale: number = Math.log(80);
	$: scale = Math.exp(logScale);

	// ── Audio engine ──────────────────────────────────────────────────────────
	let audioCtx: AudioContext | null = null;
	let activeSources: AudioBufferSourceNode[] = [];
	let audioStartTime = 0;
	let playbackStartPos = 0;

	// Keyed by raw_samples reference — stable even when clip.start/end change (drag)
	const clipBuffers = new WeakMap<number[], AudioBuffer>();

	function getAudioCtx(): AudioContext {
		if (!audioCtx) audioCtx = new AudioContext();
		return audioCtx;
	}

	async function prepareClipBuffer(clip: Clip): Promise<void> {
		if (clipBuffers.has(clip.raw_samples) || clip.sample_rate < 3000) return;

		const ctx = getAudioCtx();
		const samples = new Float32Array(clip.raw_samples);

		let buffer: AudioBuffer;
		if (clip.sample_rate === ctx.sampleRate) {
			buffer = ctx.createBuffer(1, samples.length, clip.sample_rate);
			buffer.copyToChannel(samples, 0);
		} else {
			// Resample to AudioContext native rate via OfflineAudioContext
			const duration = samples.length / clip.sample_rate;
			const offCtx = new OfflineAudioContext(1, Math.ceil(duration * ctx.sampleRate), ctx.sampleRate);
			const srcBuf = offCtx.createBuffer(1, samples.length, clip.sample_rate);
			srcBuf.copyToChannel(samples, 0);
			const srcNode = offCtx.createBufferSource();
			srcNode.buffer = srcBuf;
			srcNode.connect(offCtx.destination);
			srcNode.start();
			buffer = await offCtx.startRendering();
		}

		clipBuffers.set(clip.raw_samples, buffer);
	}

	function scheduleClip(ctx: AudioContext, clip: Clip, currentPos: number) {
		const buffer = clipBuffers.get(clip.raw_samples);
		if (!buffer || currentPos >= clip.end) return;

		const source = ctx.createBufferSource();
		source.buffer = buffer;
		source.connect(ctx.destination);

		if (currentPos < clip.start) {
			source.start(ctx.currentTime + (clip.start - currentPos));
		} else {
			source.start(0, currentPos - clip.start);
		}

		activeSources.push(source);
	}

	function stopAllClips() {
		for (const source of activeSources) {
			try { source.stop(); } catch { /* already ended */ }
		}
		activeSources = [];
	}

	// ── Playhead (position driven by audioCtx.currentTime when playing) ──────
	let animFrameId: number;

	function loop() {
		if ($transport.isPlaying && audioCtx) {
			transport.update((t) => ({
				...t,
				position: playbackStartPos + (audioCtx!.currentTime - audioStartTime)
			}));
		}
		animFrameId = requestAnimationFrame(loop);
	}

	onMount(() => {
		animFrameId = requestAnimationFrame(loop);

		let wasPlaying = false;
		const unsub = transport.subscribe((state) => {
			if (state.isPlaying && !wasPlaying) {
				const ctx = getAudioCtx();
				if (ctx.state === 'suspended') ctx.resume();
				playbackStartPos = state.position;
				audioStartTime = ctx.currentTime;
				for (const track of tracks) {
					for (const clip of track.clips) {
						scheduleClip(ctx, clip, state.position);
					}
				}
			} else if (!state.isPlaying && wasPlaying) {
				stopAllClips();
			}
			wasPlaying = state.isPlaying;
		});

		// ── File drag-and-drop via Tauri events ───────────────────────────────
		// Tauri v2 drag-* events deliver PhysicalPosition (window-relative,
		// physical px). Divide by devicePixelRatio to get CSS px, then offset
		// against the tracks container's client rect.
		function updateDragPreview(physX: number, physY: number) {
			if (!tracksEl) return;
			const dpr = window.devicePixelRatio || 1;
			const winX = physX / dpr;
			const winY = physY / dpr;
			const rect = tracksEl.getBoundingClientRect();
			const x = winX - rect.left;
			const y = winY - rect.top + tracksEl.scrollTop;
			if (x < 0 || x > rect.width || y < 0) { fileDragPreview = null; return; }
			const trackIndex = Math.min(Math.max(0, Math.floor(y / 100)), tracks.length - 1);
			fileDragPreview = { trackIndex, startSec: Math.max(0, x / scale), durationSec: pendingDragDuration ?? fileDragPreview?.durationSec ?? null };
		}

		type DragPayload = { paths: string[]; position: { x: number; y: number } };
		const unlisteners: (() => void)[] = [];
		Promise.all([
			listen<DragPayload>('tauri://drag-enter', ({ payload }) => {
				pendingDragDuration = null;
				updateDragPreview(payload.position.x, payload.position.y);
				// Fetch duration from WAV header immediately so ghost has correct width
				const path = payload.paths[0];
				if (path) {
					invoke<number>('get_audio_clip_duration', { filePath: path as string })
						.then((dur) => {
							pendingDragDuration = dur;
							if (fileDragPreview) fileDragPreview = { ...fileDragPreview, durationSec: dur };
						})
						.catch(() => {});
				}
			}),
			listen<DragPayload>('tauri://drag-over', ({ payload }) => {
				updateDragPreview(payload.position.x, payload.position.y);
			}),
			listen('tauri://drag-leave', () => { fileDragPreview = null; pendingDragDuration = null; }),
			listen<DragPayload>('tauri://drag-drop', ({ payload }) => {
				updateDragPreview(payload.position.x, payload.position.y);
				const preview = fileDragPreview;
				const filePath = payload.paths[0];
				pendingDragDuration = null;
				if (!filePath) { fileDragPreview = null; return; }
				createNewAudioClip(filePath)
					.then(async (data) => {
						const durationSec = data.raw_samples.length / data.sample_rate;
						// Update ghost width to match actual clip — stays visible during buffer prep
						if (fileDragPreview) fileDragPreview = { ...fileDragPreview, durationSec };
						const clip: Clip = {
							id: crypto.randomUUID(),
							start: preview?.startSec ?? 0,
							end: (preview?.startSec ?? 0) + durationSec,
							raw_samples: data.raw_samples,
							sample_rate: data.sample_rate,
							peaks_64: data.peaks_64,
							peaks_512: data.peaks_512,
							peaks_4096: data.peaks_4096
						};
						await prepareClipBuffer(clip);
						// Batch: clip appears + ghost disappears in the same DOM update
						const trackIndex = preview?.trackIndex ?? 0;
						tracks[trackIndex].clips = [...tracks[trackIndex].clips, clip];
						tracks = tracks;
						fileDragPreview = null;
					})
					.catch((err) => { console.error('Erreur import fichier audio :', err); fileDragPreview = null; });
			}),
		]).then((fns) => unlisteners.push(...fns));

		return () => {
			cancelAnimationFrame(animFrameId);
			unsub();
			stopAllClips();
			unlisteners.forEach((fn) => fn());
		};
	});

	// ── Time ruler (bars & beats) ─────────────────────────────────────────────
	let rulerMajorTicks: { x: number; label: string }[] = [];
	let rulerBeatTicks: { x: number; label: string }[] = [];
	let rulerMinorTicks: { x: number }[] = [];

	$: {
		const bpm = $tempo.bpm;
		const numerator = $tempo.numerator;
		const beatSec = 60 / bpm;
		const barSec = numerator * beatSec;
		const pxPerBar = barSec * scale;
		const pxPerBeat = beatSec * scale;
		const maxBars = Math.min(Math.ceil(1800 / barSec) + 4, 2000);

		const majors: typeof rulerMajorTicks = [];
		const beats: typeof rulerBeatTicks = [];
		const minors: typeof rulerMinorTicks = [];

		// Smallest power-of-2 N such that bar labels are ≥40 px apart
		let barLabelEvery = 1;
		while (barLabelEvery * pxPerBar < 40 && barLabelEvery < 4096) barLabelEvery *= 2;

		// Show beat labels (e.g. "3.2") only when individual bars are labeled
		// and each beat occupies ≥20 px
		const showBeatLabels = barLabelEvery === 1 && pxPerBeat >= 20;

		for (let bar = 1; bar <= maxBars; bar++) {
			const barT = (bar - 1) * barSec;
			const barX = barT * scale;

			if ((bar - 1) % barLabelEvery === 0) {
				majors.push({ x: barX, label: `${bar}` });
			} else if (pxPerBar >= 10) {
				minors.push({ x: barX });
			}

			// Beat ticks within the bar
			if (pxPerBeat >= 6) {
				for (let beat = 2; beat <= numerator; beat++) {
					const beatX = (barT + (beat - 1) * beatSec) * scale;
					if (showBeatLabels) {
						beats.push({ x: beatX, label: `${bar}.${beat}` });
					} else {
						minors.push({ x: beatX });
					}
				}
			}
		}

		rulerMajorTicks = majors;
		rulerBeatTicks = beats;
		rulerMinorTicks = minors;
	}

	$: playheadX = $transport.position * scale;

	let tracksEl: HTMLElement;
	let dragging: { trackIndex: number; clipIndex: number; offsetPx: number } | null = null;
	let fileDragPreview: { trackIndex: number; startSec: number; durationSec: number | null } | null = null;
	let pendingDragDuration: number | null = null;

	let scrubOriginLeft = 0;

	// Snaps positionSec to the nearest beat if within 10px; Alt key bypasses snap.
	function snapPosition(positionSec: number, altKey: boolean): number {
		if (altKey) return positionSec;
		const beatSec = 60 / $tempo.bpm;
		const thresholdSec = 10 / scale;
		const nearest = Math.round(positionSec / beatSec) * beatSec;
		return Math.abs(nearest - positionSec) <= thresholdSec ? nearest : positionSec;
	}

	function onScrubMove(e: MouseEvent) {
		const raw = Math.max(0, (e.clientX - scrubOriginLeft) / scale);
		transport.update((t) => ({ ...t, position: snapPosition(raw, e.altKey) }));
	}

	function onScrubEnd() {
		window.removeEventListener('mousemove', onScrubMove);
		window.removeEventListener('mouseup', onScrubEnd);
	}

	function handleRulerMouseDown(event: MouseEvent) {
		const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
		scrubOriginLeft = rect.left;
		const raw = Math.max(0, (event.clientX - rect.left) / scale);
		transport.update((t) => ({ ...t, position: snapPosition(raw, event.altKey) }));
		window.addEventListener('mousemove', onScrubMove);
		window.addEventListener('mouseup', onScrubEnd);
	}

	function handleTracksMouseDown(event: MouseEvent) {
		// onClipDragStart fires synchronously before this event bubbles here,
		// so if dragging is set the click was on a clip — don't seek.
		if (dragging) return;
		const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
		scrubOriginLeft = rect.left;
		const raw = Math.max(0, (event.clientX - rect.left) / scale);
		transport.update((t) => ({ ...t, position: snapPosition(raw, event.altKey) }));
		selection.set(new Set());
		window.addEventListener('mousemove', onScrubMove);
		window.addEventListener('mouseup', onScrubEnd);
	}

	function deleteSelected() {
		tracks = tracks.map((track) => ({
			...track,
			clips: track.clips.filter((c) => !$selection.has(c.id))
		}));
		selection.set(new Set());
	}

	function handleKeyDown(e: KeyboardEvent) {
		if (e.key !== 'Delete' && e.key !== 'Backspace') return;
		const target = e.target as HTMLElement;
		if (target.isContentEditable || target.tagName === 'INPUT') return;
		deleteSelected();
	}

	function onClipDragStart(e: CustomEvent<{ offsetPx: number; trackIndex: number; clipIndex: number }>) {
		dragging = e.detail;
		window.addEventListener('mousemove', onDragMove);
		window.addEventListener('mouseup', onDragEnd);
	}

	function onDragMove(e: MouseEvent) {
		if (!dragging || !tracksEl) return;
		const rect = tracksEl.getBoundingClientRect();
		const newStart = Math.max(0, (e.clientX - rect.left - dragging.offsetPx) / scale);
		const { trackIndex, clipIndex } = dragging;
		const clip = tracks[trackIndex].clips[clipIndex];
		const duration = clip.end - clip.start;
		tracks[trackIndex].clips[clipIndex] = { ...clip, start: newStart, end: newStart + duration };
		tracks = tracks;
	}

	function onDragEnd() {
		dragging = null;
		window.removeEventListener('mousemove', onDragMove);
		window.removeEventListener('mouseup', onDragEnd);
	}

	// Tauri intercepts OS file drops before WebView2 sees them — these stubs
	// just prevent the browser's default "open file" fallback on non-Tauri drags.
	function handleDragEnter(event: DragEvent) { event.preventDefault(); }
	function handleDragLeave(event: DragEvent) { event.preventDefault(); }
	function handleDragOver(event: DragEvent) { event.preventDefault(); }
	function handleDrop(event: DragEvent) { event.preventDefault(); }

	interface AudioClipData {
		raw_samples: number[];
		sample_rate: number;
		peaks_64: [number, number][];
		peaks_512: [number, number][];
		peaks_4096: [number, number][];
	}

	function createNewAudioClip(filePath: string): Promise<AudioClipData> {
		return invoke('create_new_audio_clip', { filePath });
	}

	async function onClick() {
		const filePath = await open({
			filters: [{ name: 'Audio', extensions: ['wav'] }]
		});
		if (!filePath || Array.isArray(filePath)) return;

		createNewAudioClip(filePath as string)
			.then(async (data) => {
				const clip: Clip = {
					id: crypto.randomUUID(),
					start: 0,
					end: data.raw_samples.length / data.sample_rate,
					raw_samples: data.raw_samples,
					sample_rate: data.sample_rate,
					peaks_64: data.peaks_64,
					peaks_512: data.peaks_512,
					peaks_4096: data.peaks_4096
				};
				await prepareClipBuffer(clip);
				tracks[0].clips = [...tracks[0].clips, clip];
			})
			.catch((error) => console.error('Erreur:', error));
	}

	function makeEditable(this: HTMLElement) {
		this.contentEditable = 'true';
		this.focus();

		this.addEventListener('keydown', function (event) {
			if (event.key === 'Enter') {
				event.preventDefault();
				this.blur();
			}
		});

		this.addEventListener('blur', function () {
			this.contentEditable = 'false';
		});
	}
</script>

<svelte:window on:keydown={handleKeyDown} />

<div class="container">
	<input type="range" bind:value={logScale} min={Math.log(10)} max={Math.log(2000)} step={0.01} class="scale" />

	<div class="left-panel">
		<div class="ruler-spacer"></div>
		<div class="instruments">
			{#each tracks as { name, volume }}
				<div class="instrument">
					<strong on:dblclick={makeEditable}>{name}</strong>
					<Knob min={0} max={100} bind:value={volume} />
				</div>
			{/each}
		</div>
	</div>

	<div class="right-panel">
		<!-- svelte-ignore a11y-no-static-element-interactions a11y-click-events-have-key-events -->
		<div class="ruler" on:mousedown={handleRulerMouseDown}>
			{#each rulerMinorTicks as tick}
				<div class="tick tick--minor" style="left:{tick.x}px"></div>
			{/each}
			{#each rulerBeatTicks as tick}
				<div class="tick tick--beat" style="left:{tick.x}px">
					<span class="tick-label tick-label--beat">{tick.label}</span>
				</div>
			{/each}
			{#each rulerMajorTicks as tick}
				<div class="tick tick--major" style="left:{tick.x}px">
					<span class="tick-label">{tick.label}</span>
				</div>
			{/each}
		</div>

		<div class="playhead" style="transform: translateX({playheadX}px)"></div>

		<!-- svelte-ignore a11y-no-static-element-interactions a11y-click-events-have-key-events -->
		<div
			class="tracks"
			class:is-dragging={dragging}
			class:is-file-dragging={fileDragPreview}
			bind:this={tracksEl}
			on:drop={handleDrop}
			on:dragover={handleDragOver}
			on:dragenter={handleDragEnter}
			on:dragleave={handleDragLeave}
			on:mousedown={handleTracksMouseDown}
		>
			{#if fileDragPreview}
				<div
					class="clip-ghost"
					style="top: {fileDragPreview.trackIndex * 100}px; left: {fileDragPreview.startSec * scale}px; width: {fileDragPreview.durationSec != null ? fileDragPreview.durationSec * scale : 150}px;"
				></div>
			{/if}
			{#each tracks as { clips }, trackIndex}
				<Track {clips} {scale} {trackIndex} on:clipdragstart={onClipDragStart} />
			{/each}
		</div>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.container {
		display: flex;
		height: 100%;
		position: relative;
		overflow: hidden;
	}

	.scale {
		position: absolute;
		right: 5px;
		bottom: 5px;
		z-index: 20;
	}

	// ── Left panel ────────────────────────────────────────────────────────────
	.left-panel {
		width: 200px;
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		background-color: $pannel-background-color;
	}

	.ruler-spacer {
		height: 22px;
		flex-shrink: 0;
		background-color: $background-color;
		border-bottom: 1px solid $border-color;
		border-right: 1px solid $border-color;
	}

	.instruments {
		flex: 1;
		overflow-y: auto;
	}

	// ── Right panel (ruler + tracks, playhead spans both) ─────────────────────
	.right-panel {
		flex: 1;
		display: flex;
		flex-direction: column;
		position: relative;
		overflow: hidden;
	}

	.ruler {
		height: 22px;
		flex-shrink: 0;
		position: relative;
		overflow: hidden;
		background-color: $background-color;
		border-bottom: 1px solid $border-color;
		cursor: pointer;
		user-select: none;
	}

	.tick {
		position: absolute;
		bottom: 0;
		width: 1px;

		&--major {
			height: 14px;
			background-color: rgba(247, 247, 249, 0.45);
		}

		&--minor {
			height: 7px;
			background-color: rgba(247, 247, 249, 0.18);
		}

		&--beat {
			height: 7px;
			background-color: rgba(247, 247, 249, 0.18);
		}
	}

	.tick-label {
		position: absolute;
		bottom: 2px;
		left: 4px;
		font-size: 10px;
		line-height: 1;
		color: rgba(247, 247, 249, 0.55);
		white-space: nowrap;
		font-family: 'SF Mono', 'Fira Code', ui-monospace, monospace;
		pointer-events: none;

		&--beat {
			font-size: 9px;
			color: rgba(247, 247, 249, 0.3);
		}
	}

	.tracks {
		flex: 1;
		overflow-y: auto;
		overflow-x: hidden;
		position: relative;
		cursor: crosshair;

		&.is-dragging {
			cursor: grabbing;
			user-select: none;
		}

		&.is-file-dragging {
			cursor: copy;
		}
	}

	.clip-ghost {
		position: absolute;
		box-sizing: border-box;
		height: 100px;
		background-color: rgba(21, 113, 255, 0.15);
		border-radius: 4px;
		pointer-events: none;
		border: 2px dashed rgba(255, 255, 255, 0.6);
		z-index: 5;
	}

	.instrument {
		height: 100px;
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 10px;
		box-sizing: border-box;
	}

	// Playhead is positioned in right-panel so it spans ruler + tracks
	.playhead {
		position: absolute;
		top: 0;
		left: 0;
		width: 2px;
		height: 100%;
		background-color: #fff;
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
			border-top: 8px solid #fff;
		}
	}
</style>
