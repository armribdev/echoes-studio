<script lang="ts">
	import Knob from './Knob.svelte';
	import Track from './Track.svelte';
	import type { Track as TrackType, Clip, Project } from '../types/global';
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { onMount, onDestroy } from 'svelte';
	import { transport } from '../stores/transport';
	import { selection } from '../stores/selection';
	import { tempo } from '../stores/tempo';
	import { projectName, projectPath } from '../stores/project';
	import { projectEventStore } from '../shared/project/eventStore';
	import { addClip, moveClip, removeClip, updateTrack, muteTrack, soloTrack, seek } from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';
	import { exportTrigger } from '../stores/exportBus';
	import { save } from '@tauri-apps/plugin-dialog';

	// Live sync from the engine event store.
	// Local mutations (optimistic UI during drag/import) are reconciled by the
	// next store event — the reducer is idempotent so there's never a double-apply.
	let tracks: TrackType[] = [];
	const unsubStore = projectEventStore.subscribe((state) => {
		tracks = state.tracks;
		// Keep the tempo store in sync so MenuBar reflects undo/redo correctly.
		tempo.set({ bpm: state.bpm, numerator: state.numerator, denominator: state.denominator });
	});
	onDestroy(unsubStore);

	// ── Export trigger ────────────────────────────────────────────────────────
	const unsubExport = exportTrigger.subscribe((n) => {
		if (n === 0) return;
		exportMixdown().catch((e: unknown) =>
			notify(`Erreur export : ${(e as Error)?.message ?? String(e)}`, 'error')
		);
	});
	onDestroy(unsubExport);

	let logScale: number = Math.log(80);
	$: scale = Math.exp(logScale);

	// ── Audio engine ──────────────────────────────────────────────────────────
	let audioCtx: AudioContext | null = null;
	let activeSources: AudioBufferSourceNode[] = [];
	let audioStartTime = 0;
	let playbackStartPos = 0;

	// Keyed by clip.id — stable across undo/redo and object recreation by the reducer.
	// (WeakMap keyed by raw_samples breaks when the reducer spreads new objects.)
	const clipBuffers = new Map<string, AudioBuffer>();

	// One GainNode + StereoPannerNode per track — persists across play/stop cycles.
	const trackGains = new Map<string, GainNode>();
	const trackPanners = new Map<string, StereoPannerNode>();

	function panToAudioValue(pan: number): number {
		return pan / 100;
	}

	function getEffectiveGain(track: TrackType): number {
		const anySolo = tracks.some((t) => t.solo);
		if (track.muted) return 0;
		if (anySolo && !track.solo) return 0;
		return track.volume / 100;
	}

	// Reactive: keep gain/pan in sync with controls (even during playback).
	$: if (audioCtx) {
		for (const track of tracks) {
			const gain = trackGains.get(track.id);
			if (gain) gain.gain.value = getEffectiveGain(track);
			const panner = trackPanners.get(track.id);
			if (panner) panner.pan.value = panToAudioValue(track.pan);
		}
	}

	// Reactive: auto-prepare AudioBuffers for every clip in `tracks`.
	// Fires on project load, gateway imports, and undo/redo restores.
	// prepareClipBuffer is idempotent — no-ops if the buffer is already cached.
	$: for (const track of tracks) {
		for (const clip of track.clips) {
			if (!clipBuffers.has(clip.id) && clip.raw_samples.length > 0) {
				prepareClipBuffer(clip).catch(console.error);
			}
		}
	}

	function getAudioCtx(): AudioContext {
		if (!audioCtx) audioCtx = new AudioContext();
		return audioCtx;
	}

	async function prepareClipBuffer(clip: Clip): Promise<void> {
		if (clipBuffers.has(clip.id) || clip.sample_rate < 3000) return;

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

		clipBuffers.set(clip.id, buffer);
	}

	function getOrCreateTrackGain(ctx: AudioContext, trackId: string, gainValue: number, pan: number): GainNode {
		let gain = trackGains.get(trackId);
		let panner = trackPanners.get(trackId);
		if (!gain) {
			gain = ctx.createGain();
			panner = ctx.createStereoPanner();
			gain.connect(panner);
			panner.connect(ctx.destination);
			trackGains.set(trackId, gain);
			trackPanners.set(trackId, panner);
		}
		gain.gain.value = gainValue;
		panner!.pan.value = panToAudioValue(pan);
		return gain;
	}

	function scheduleClip(ctx: AudioContext, clip: Clip, trackId: string, gainValue: number, pan: number, currentPos: number) {
		const buffer = clipBuffers.get(clip.id);
		if (!buffer || currentPos >= clip.end) return;

		const gain = getOrCreateTrackGain(ctx, trackId, gainValue, pan);
		const source = ctx.createBufferSource();
		source.buffer = buffer;
		source.connect(gain);

		if (currentPos < clip.start) {
			source.start(ctx.currentTime + (clip.start - currentPos));
		} else {
			source.start(0, currentPos - clip.start);
		}

		activeSources.push(source);
	}

	function scheduleAllClips(ctx: AudioContext, currentPos: number) {
		for (const track of tracks) {
			const gainValue = getEffectiveGain(track);
			for (const clip of track.clips) {
				scheduleClip(ctx, clip, track.id, gainValue, track.pan, currentPos);
			}
		}
	}

	function startPlaybackAt(ctx: AudioContext, position: number) {
		if (ctx.state === 'suspended') ctx.resume();
		playbackStartPos = position;
		audioStartTime = ctx.currentTime;
		scheduleAllClips(ctx, position);
	}

	function stopAllClips() {
		for (const source of activeSources) {
			try { source.stop(); } catch { /* already ended */ }
		}
		activeSources = [];
	}

	// ── Project loading ───────────────────────────────────────────────────────

	async function loadProject(dto: Project) {
		stopAllClips();
		transport.update((t) => ({ ...t, isPlaying: false, position: 0 }));

		projectName.set(dto.name);
		tempo.set({ bpm: dto.bpm, numerator: dto.numerator, denominator: dto.denominator });

		// Seed the engine store — the subscription above will update `tracks` reactively,
		// which in turn triggers the $: buffer-preparation block for all clips.
		projectEventStore.seedFromSnapshot(dto);

		// Window title — set from frontend so the window is guaranteed to exist
		getCurrentWindow().setTitle(`${dto.name} — Echoes Studio`).catch(console.error);
	}

	// ── Playhead (position driven by audioCtx.currentTime when playing) ──────
	let animFrameId: number;
	let isScrubbing = false;

	function loop() {
		if ($transport.isPlaying && audioCtx && !isScrubbing) {
			transport.update((t) => ({
				...t,
				position: playbackStartPos + (audioCtx!.currentTime - audioStartTime)
			}));
		}
		animFrameId = requestAnimationFrame(loop);
	}

	onMount(async () => {
		// Hydrate from Rust state
		const dto = await invoke<Project>('get_project');
		await loadProject(dto);

		animFrameId = requestAnimationFrame(loop);

		let wasPlaying = false;
		const unsub = transport.subscribe((state) => {
			if (state.isPlaying && !wasPlaying) {
				startPlaybackAt(getAudioCtx(), state.position);
			} else if (!state.isPlaying && wasPlaying) {
				stopAllClips();
			}
			wasPlaying = state.isPlaying;
		});

		// ── File drag-and-drop via Tauri events ───────────────────────────────
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
				const path = payload.paths[0];
				if (path) {
					invoke<number>('get_audio_clip_duration', { filePath: path })
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
				const knownDuration = pendingDragDuration ?? preview?.durationSec ?? null;
				pendingDragDuration = null;
				if (!filePath) { fileDragPreview = null; return; }

				const trackIndex = preview?.trackIndex ?? 0;
				const startSec = preview?.startSec ?? 0;

				const doImport = (dur: number) => {
					const trackId = tracks[trackIndex]?.id;
					if (!trackId) { fileDragPreview = null; return; }

					// Dispatch through the gateway — Rust loads the WAV, emits CLIP_ADDED,
					// the store subscription updates `tracks`, and the $: reactive block
					// prepares the AudioBuffer automatically.
					addClip(trackId, filePath, startSec, startSec + dur)
						.then(() => { fileDragPreview = null; })
						.catch((err) => { notify(`Erreur import : ${err?.message ?? err}`, 'error'); fileDragPreview = null; });
				};

				if (knownDuration !== null) {
					if (fileDragPreview) fileDragPreview = { ...fileDragPreview, durationSec: knownDuration };
					doImport(knownDuration);
				} else {
					invoke<number>('get_audio_clip_duration', { filePath })
						.then((dur) => {
							if (fileDragPreview) fileDragPreview = { ...fileDragPreview, durationSec: dur };
							doImport(dur);
						})
						.catch((err) => { notify(`Erreur import : ${err?.message ?? err}`, 'error'); fileDragPreview = null; });
				}
			}),
		]).then((fns) => unlisteners.push(...fns));

		// ── Native menu events (project-loaded / project-saved / audio import) ─
		Promise.all([
			listen<Project>('project-loaded', async ({ payload }) => {
				await loadProject(payload);
				notify(`Projet « ${payload.name} » chargé`, 'success');
			}),
			listen<string>('project-saved', ({ payload }) => {
				projectPath.set(payload);
				const filename = payload.split(/[\\/]/).pop() ?? payload;
				notify(`Enregistré : ${filename}`, 'success');
			}),
			listen<string>('audio-file-picked', async ({ payload: filePath }) => {
				const targetTrack = tracks[0];
				if (!targetTrack) {
					notify('Créez d\'abord une piste avant d\'importer.', 'error');
					return;
				}
				try {
					const dur = await invoke<number>('get_audio_clip_duration', { filePath });
					const startSec = $transport.position;
					await addClip(targetTrack.id, filePath, startSec, startSec + dur);
					const filename = filePath.split(/[\\/]/).pop() ?? filePath;
					notify(`Importé : ${filename}`, 'success');
				} catch (err: unknown) {
					const msg = err instanceof Error ? err.message : String(err);
					notify(`Erreur import : ${msg}`, 'error');
				}
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

		let barLabelEvery = 1;
		while (barLabelEvery * pxPerBar < 40 && barLabelEvery < 4096) barLabelEvery *= 2;

		const showBeatLabels = barLabelEvery === 1 && pxPerBeat >= 20;

		for (let bar = 1; bar <= maxBars; bar++) {
			const barT = (bar - 1) * barSec;
			const barX = barT * scale;

			if ((bar - 1) % barLabelEvery === 0) {
				majors.push({ x: barX, label: `${bar}` });
			} else if (pxPerBar >= 10) {
				minors.push({ x: barX });
			}

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
	let scrubPosition = 0;

	// Returns the finest musical subdivision (in seconds) that still spans
	// at least MIN_SNAP_PX pixels at the current zoom level.
	function getSnapSubdivisionSec(): number {
		const MIN_SNAP_PX = 8;
		const beatSec = 60 / $tempo.bpm;
		const barSec = $tempo.numerator * beatSec;

		// Finer than a beat: 1/2, 1/4, 1/8, 1/16, 1/32
		for (const denom of [32, 16, 8, 4, 2, 1]) {
			const subSec = beatSec / denom;
			if (subSec * scale >= MIN_SNAP_PX) return subSec;
		}

		// Coarser than a beat: whole bars, then every 2, 4, 8 … bars
		let multiple = 1;
		while (barSec * multiple * scale < MIN_SNAP_PX) multiple *= 2;
		return barSec * multiple;
	}

	function snapPosition(
		positionSec: number,
		altKey: boolean,
		excludeClip?: { trackIndex: number; clipIndex: number }
	): number {
		if (altKey) return positionSec;
		const thresholdSec = 10 / scale;

		const candidates: number[] = [];

		// Adaptive beat grid
		const subdivSec = getSnapSubdivisionSec();
		candidates.push(Math.round(positionSec / subdivSec) * subdivSec);

		// Edges of other clips
		for (let ti = 0; ti < tracks.length; ti++) {
			const clips = tracks[ti].clips;
			for (let ci = 0; ci < clips.length; ci++) {
				if (excludeClip && ti === excludeClip.trackIndex && ci === excludeClip.clipIndex) continue;
				candidates.push(clips[ci].start);
				candidates.push(clips[ci].end);
			}
		}

		let best = positionSec;
		let bestDist = thresholdSec;
		for (const c of candidates) {
			const d = Math.abs(c - positionSec);
			if (d < bestDist) {
				bestDist = d;
				best = c;
			}
		}
		return best;
	}

	function onScrubMove(e: MouseEvent) {
		const raw = Math.max(0, (e.clientX - scrubOriginLeft) / scale);
		scrubPosition = snapPosition(raw, e.altKey);
		transport.update((t) => ({ ...t, position: scrubPosition }));
	}

	function onScrubEnd() {
		window.removeEventListener('mousemove', onScrubMove);
		window.removeEventListener('mouseup', onScrubEnd);
		isScrubbing = false;
		if ($transport.isPlaying) {
			stopAllClips();
			startPlaybackAt(getAudioCtx(), scrubPosition);
		}
		seek(scrubPosition).catch(console.error);
	}

	function beginScrub(event: MouseEvent, originEl: HTMLElement) {
		const rect = originEl.getBoundingClientRect();
		scrubOriginLeft = rect.left;
		const raw = Math.max(0, (event.clientX - rect.left) / scale);
		scrubPosition = snapPosition(raw, event.altKey);
		isScrubbing = true;
		transport.update((t) => ({ ...t, position: scrubPosition }));
		window.addEventListener('mousemove', onScrubMove);
		window.addEventListener('mouseup', onScrubEnd);
	}

	function handleRulerMouseDown(event: MouseEvent) {
		beginScrub(event, event.currentTarget as HTMLElement);
	}

	function handleTracksMouseDown(event: MouseEvent) {
		if (dragging) return;
		beginScrub(event, event.currentTarget as HTMLElement);
		selection.set(new Set());
	}

	function deleteSelected() {
		// Optimistic: remove from local state immediately for instant feedback.
		// The CLIP_REMOVED events will confirm via store subscription.
		tracks.forEach((track) => {
			const toDelete = track.clips.filter((c) => $selection.has(c.id));
			for (const clip of toDelete) {
				removeClip(track.id, clip.id).catch(console.error);
			}
		});
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
		const { trackIndex, clipIndex } = dragging;
		const rawStart = Math.max(0, (e.clientX - rect.left - dragging.offsetPx) / scale);
		const newStart = snapPosition(rawStart, e.altKey, { trackIndex, clipIndex });
		const clip = tracks[trackIndex].clips[clipIndex];
		const duration = clip.end - clip.start;
		tracks[trackIndex].clips[clipIndex] = { ...clip, start: newStart, end: newStart + duration };
		tracks = tracks;
	}

	function onDragEnd() {
		if (dragging) {
			const { trackIndex, clipIndex } = dragging;
			const track = tracks[trackIndex];
			const clip = track?.clips[clipIndex];
			if (track && clip) {
				moveClip(track.id, clip.id, clip.start, clip.end).catch(console.error);
			}
		}
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

	function handleVolumeRelease(trackIndex: number) {
		const track = tracks[trackIndex];
		if (!track) return;
		updateTrack(track.id, { name: track.name, volume: track.volume }).catch(console.error);
	}

	function handlePanRelease(trackIndex: number) {
		const track = tracks[trackIndex];
		if (!track) return;
		updateTrack(track.id, { pan: track.pan }).catch(console.error);
	}

	function handleMuteToggle(trackIndex: number) {
		const track = tracks[trackIndex];
		if (!track) return;
		muteTrack(track.id, !track.muted).catch(console.error);
	}

	function handleSoloToggle(trackIndex: number) {
		const track = tracks[trackIndex];
		if (!track) return;
		soloTrack(track.id, !track.solo).catch(console.error);
	}

	function makeEditable(e: MouseEvent, trackIndex: number) {
		const el = e.currentTarget;
		if (!(el instanceof HTMLElement)) return;

		el.contentEditable = 'true';
		el.focus();

		el.addEventListener('keydown', function (event) {
			if (event.key === 'Enter') {
				event.preventDefault();
				el.blur();
			}
		});

		el.addEventListener('blur', function () {
			el.contentEditable = 'false';
			const newName = el.textContent?.trim() || tracks[trackIndex].name;
			tracks[trackIndex] = { ...tracks[trackIndex], name: newName };
			tracks = tracks;
			updateTrack(tracks[trackIndex].id, { name: newName, volume: tracks[trackIndex].volume }).catch(console.error);
		}, { once: true });
	}

	// ── Export mixdown ────────────────────────────────────────────────────────

	async function exportMixdown(): Promise<void> {
		// 1. Calculate total project duration
		let totalDuration = 0;
		for (const track of tracks) {
			for (const clip of track.clips) {
				totalDuration = Math.max(totalDuration, clip.end);
			}
		}
		if (totalDuration <= 0) {
			notify('Aucun clip à exporter', 'error');
			return;
		}

		// 2. Save dialog (before the expensive render)
		const filePath = await save({
			filters: [{ name: 'Fichier WAV', extensions: ['wav'] }],
			defaultPath: 'mixdown.wav',
		});
		if (!filePath) return;

		notify('Export en cours…', 'info');

		// 3. Offline render — same graph as live: GainNode → StereoPannerNode
		const sampleRate = audioCtx?.sampleRate ?? 44100;
		const length = Math.ceil(totalDuration * sampleRate);
		const offCtx = new OfflineAudioContext(2, length, sampleRate);

		const anySolo = tracks.some((t) => t.solo);

		for (const track of tracks) {
			let effectiveGain = track.volume / 100;
			if (track.muted || (anySolo && !track.solo)) effectiveGain = 0;

			const gainNode = offCtx.createGain();
			gainNode.gain.value = effectiveGain;
			const panNode = offCtx.createStereoPanner();
			panNode.pan.value = track.pan / 100;
			gainNode.connect(panNode);
			panNode.connect(offCtx.destination);

			for (const clip of track.clips) {
				if (clip.raw_samples.length === 0) continue;

				const samples = new Float32Array(clip.raw_samples);
				let clipBuffer: AudioBuffer;

				if (clip.sample_rate === sampleRate) {
					clipBuffer = offCtx.createBuffer(1, samples.length, clip.sample_rate);
					clipBuffer.copyToChannel(samples, 0);
				} else {
					// Resample to offCtx sample rate via a nested OfflineAudioContext
					const duration = samples.length / clip.sample_rate;
					const resCtx = new OfflineAudioContext(
						1,
						Math.ceil(duration * sampleRate),
						sampleRate
					);
					const srcBuf = resCtx.createBuffer(1, samples.length, clip.sample_rate);
					srcBuf.copyToChannel(samples, 0);
					const srcNode = resCtx.createBufferSource();
					srcNode.buffer = srcBuf;
					srcNode.connect(resCtx.destination);
					srcNode.start();
					clipBuffer = await resCtx.startRendering();
				}

				const source = offCtx.createBufferSource();
				source.buffer = clipBuffer;
				source.connect(gainNode);
				source.start(clip.start);
			}
		}

		const rendered = await offCtx.startRendering();
		const samplesLeft = Array.from(rendered.getChannelData(0));
		const samplesRight = rendered.numberOfChannels > 1
			? Array.from(rendered.getChannelData(1))
			: samplesLeft;

		// 4. Send to Rust for WAV writing
		await invoke('export_wav', {
			path: filePath,
			samplesLeft,
			samplesRight,
			sampleRate,
		});

		const filename = (filePath as string).split(/[\\/]/).pop() ?? 'mixdown.wav';
		notify(`Exporté : ${filename}`, 'success');
	}


</script>

<svelte:window on:keydown={handleKeyDown} />

<div class="container">
	<input type="range" bind:value={logScale} min={Math.log(10)} max={Math.log(2000)} step={0.01} class="scale" />

	<div class="left-panel">
		<div class="ruler-spacer"></div>
		<div class="instruments">
			{#each tracks as track, trackIndex}
				<div class="instrument">
					<!-- svelte-ignore a11y-no-static-element-interactions -->
					<strong on:dblclick={(e) => makeEditable(e, trackIndex)}>{track.name}</strong>
					<div class="track-controls">
						<button
							type="button"
							class="ms-btn"
							class:active={track.muted}
							title="Mute"
							on:click={() => handleMuteToggle(trackIndex)}
						>M</button>
						<button
							type="button"
							class="ms-btn"
							class:active={track.solo}
							title="Solo"
							on:click={() => handleSoloToggle(trackIndex)}
						>S</button>
						<Knob min={0} max={100} bind:value={tracks[trackIndex].volume} on:release={() => handleVolumeRelease(trackIndex)} />
						<Knob min={-100} max={100} bind:value={tracks[trackIndex].pan} on:release={() => handlePanRelease(trackIndex)} />
					</div>
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

	.track-controls {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.ms-btn {
		width: 22px;
		height: 22px;
		padding: 0;
		border: 1px solid $border-color;
		border-radius: 3px;
		background: transparent;
		color: rgba($text-color, 0.55);
		font-size: 11px;
		font-weight: 600;
		line-height: 1;
		cursor: pointer;

		&:hover {
			color: $text-color;
			border-color: rgba($text-color, 0.4);
		}

		&.active {
			background: $accent-color;
			border-color: $accent-color;
			color: $text-color;
		}
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
