<script lang="ts">
	import Track from './Track.svelte';
	import TrackContextMenu from './TrackContextMenu.svelte';
	import ClipContextMenu from './ClipContextMenu.svelte';
	import FaderMeter from './FaderMeter.svelte';
	import Knob from './Knob.svelte';
	import type { Track as TrackType, Project } from '../types/global';
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { onMount, onDestroy } from 'svelte';
	import { transport } from '../stores/transport';
	import { selection } from '../stores/selection';
	import { tempo } from '../stores/tempo';
	import { projectName, projectPath } from '../stores/project';
	import { projectEventStore } from '../shared/project/eventStore';
	import { addClip, moveClip, removeClip, updateTrack, muteTrack, soloTrack, seek, createTrack, deleteTrack } from '../frontend/commands/dispatcher';
	import { clipboard } from '../stores/clipboard';
	import type { ClipboardEntry } from '../stores/clipboard';
	import { get } from 'svelte/store';

	const TRACK_COLORS = [
		'#4d96ff', '#ff6b6b', '#6bcb77', '#ffd93d',
		'#c77dff', '#ff9a3c', '#00c9a7', '#ff6db6',
	];
	import { notify } from '../stores/status';
	import { exportTrigger } from '../stores/exportBus';
	import { trackLevels } from '../stores/trackLevels';
	import { preferences } from '../stores/preferences';
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

	let contextMenu: { x: number; y: number; trackId: string; trackName: string; trackIndex: number } | null = null;
	let clipContextMenu: { x: number; y: number } | null = null;
	let selectedTrackIndex: number | null = null;

	let logScale: number = Math.log(80);
	$: scale = Math.exp(logScale);

	// ── Audio engine (native CPAL) ────────────────────────────────────────────
	//
	// Live audio playback is handled entirely by the Rust CPAL engine.
	// The frontend no longer creates an AudioContext for playback; it only
	// uses OfflineAudioContext for the offline WAV export path below.
	//
	// Position updates arrive via `POSITION_UPDATED` engine events pushed by
	// the Rust position-reporter thread (~30 fps) and are applied by the
	// transport bridge (src/frontend/transport/bridge.ts).

	// ── Project loading ───────────────────────────────────────────────────────

	async function loadProject(dto: Project) {
		transport.update((t) => ({ ...t, isPlaying: false, position: 0 }));

		projectName.set(dto.name);
		tempo.set({ bpm: dto.bpm, numerator: dto.numerator, denominator: dto.denominator });

		// Seed the engine store — the subscription above will update `tracks` reactively,
		// which in turn triggers the $: buffer-preparation block for all clips.
		projectEventStore.seedFromSnapshot(dto);

		// Window title — set from frontend so the window is guaranteed to exist
		getCurrentWindow().setTitle(`${dto.name} — Echoes Studio`).catch(console.error);
	}

	// ── Playhead ──────────────────────────────────────────────────────────────
	// Position is pushed by the CPAL engine via POSITION_UPDATED events (~30 fps).
	// The animFrame loop is kept for future per-frame UI work (e.g. VU meters).
	let animFrameId: number;
	let isScrubbing = false;

	function loop() {
		animFrameId = requestAnimationFrame(loop);
	}

	onMount(() => {
		// Cleanup handles declared up-front so the returned cleanup function can
		// reference them even if the component is destroyed before the async
		// initialization below completes.
		const unlisteners: (() => void)[] = [];
		let unsub: () => void = () => {};

		(async () => {
			// Hydrate from Rust state
			const dto = await invoke<Project>('get_project');
			await loadProject(dto);

			animFrameId = requestAnimationFrame(loop);

			// The transport store is updated by the engine-event bridge.
			// No Web Audio wiring needed here.
			let wasPlaying = false;
			unsub = transport.subscribe((state) => {
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
			const dragFns = await Promise.all([
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
			]);
			unlisteners.push(...dragFns);

			// ── Native menu events (project-loaded / project-saved / audio import) ─
			const menuFns = await Promise.all([
				listen<Project>('project-loaded', async ({ payload }) => {
					await loadProject(payload);
					notify(`Projet « ${payload.name} » chargé`, 'success');
				}),
				listen<string>('project-saved', ({ payload }) => {
					projectPath.set(payload);
					const filename = payload.split(/[\\/]/).pop() ?? payload;
					notify(`Enregistré : ${filename}`, 'success');
				}),
				listen<string>('track-action', ({ payload }) => {
					if (payload === 'add-track') {
						handleCreateTrack();
					} else if (payload === 'delete-track') {
						const idx = selectedTrackIndex ?? (tracks.length > 0 ? tracks.length - 1 : null);
						if (idx !== null && tracks[idx]) {
							handleDeleteTrack(tracks[idx].id);
						} else {
							notify('Aucune piste sélectionnée', 'error');
						}
					}
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
			]);
			unlisteners.push(...menuFns);
		})().catch(console.error);

		return () => {
			cancelAnimationFrame(animFrameId);
			unsub();
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
		// CPAL engine picks up the new position from the atomic — no Web Audio restart needed.
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

	function copySelected() {
		const sel = $selection;
		if (sel.size === 0) return;
		let minStart = Infinity;
		for (const track of tracks) {
			for (const clip of track.clips) {
				if (sel.has(clip.id)) minStart = Math.min(minStart, clip.start);
			}
		}
		const entries: ClipboardEntry[] = [];
		for (const track of tracks) {
			for (const clip of track.clips) {
				if (sel.has(clip.id)) {
					entries.push({
						trackId: track.id,
						filePath: clip.file_path,
						durationSec: clip.end - clip.start,
						relativeStart: clip.start - minStart,
					});
				}
			}
		}
		clipboard.set(entries);
	}

	function cutSelected() {
		copySelected();
		deleteSelected();
	}

	async function pasteClips() {
		const entries = get(clipboard);
		if (entries.length === 0) return;
		const pastePos = $transport.position;
		await Promise.all(
			entries.map((entry) => {
				const targetTrackId = tracks.find((t) => t.id === entry.trackId)?.id ?? tracks[0]?.id;
				if (!targetTrackId) return Promise.resolve();
				const start = pastePos + entry.relativeStart;
				return addClip(targetTrackId, entry.filePath, start, start + entry.durationSec);
			})
		);
	}

	function handleClipContextMenu(e: CustomEvent<{ id: string; trackIndex: number; x: number; y: number }>) {
		const { id, x, y } = e.detail;
		// If the right-clicked clip is not in the selection, make it the sole selection.
		if (!$selection.has(id)) {
			selection.set(new Set([id]));
		}
		contextMenu = null;
		clipContextMenu = { x, y };
	}

	function handleKeyDown(e: KeyboardEvent) {
		const target = e.target as HTMLElement;
		if (target.isContentEditable || target.tagName === 'INPUT') return;

		const ctrl = e.ctrlKey || e.metaKey;

		if ((e.key === 'Delete' || e.key === 'Backspace') && !ctrl) {
			deleteSelected();
		} else if (ctrl && !e.shiftKey && !e.altKey) {
			if (e.key === 'c') {
				e.preventDefault();
				copySelected();
			} else if (e.key === 'x') {
				e.preventDefault();
				cutSelected();
			} else if (e.key === 'v') {
				e.preventDefault();
				pasteClips().catch(console.error);
			}
		}
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

	function handleCreateTrack() {
		createTrack(`Piste ${tracks.length + 1}`).catch((e: unknown) =>
			notify(`Erreur création piste : ${(e as Error)?.message ?? String(e)}`, 'error')
		);
	}

	function handleDeleteTrack(trackId: string) {
		deleteTrack(trackId).catch((e: unknown) =>
			notify(`Erreur suppression piste : ${(e as Error)?.message ?? String(e)}`, 'error')
		);
	}

	function handleTrackContextMenu(e: MouseEvent, trackIndex: number) {
		e.preventDefault();
		e.stopPropagation();
		const track = tracks[trackIndex];
		if (!track) return;
		contextMenu = { x: e.clientX, y: e.clientY, trackId: track.id, trackName: track.name, trackIndex };
	}

	function renameTrackByIndex(trackIndex: number) {
		const nameEls = document.querySelectorAll<HTMLElement>('.track-name');
		const el = nameEls[trackIndex];
		if (!el) return;
		el.contentEditable = 'true';
		el.focus();
		const range = document.createRange();
		range.selectNodeContents(el);
		const sel = window.getSelection();
		sel?.removeAllRanges();
		sel?.addRange(range);
		el.addEventListener('keydown', function onKey(ev: KeyboardEvent) {
			if (ev.key === 'Enter') { ev.preventDefault(); el.blur(); }
		});
		el.addEventListener('blur', function() {
			el.contentEditable = 'false';
			const newName = el.textContent?.trim() || tracks[trackIndex].name;
			tracks[trackIndex] = { ...tracks[trackIndex], name: newName };
			tracks = tracks;
			updateTrack(tracks[trackIndex].id, { name: newName, volume: tracks[trackIndex].volume }).catch(console.error);
		}, { once: true });
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

	// ── Zoom controls ─────────────────────────────────────────────────────────
	const LOG_MIN = Math.log(10);
	const LOG_MAX = Math.log(2000);
	const ZOOM_STEP = 0.12;

	function zoomIn() {
		logScale = Math.min(LOG_MAX, logScale + ZOOM_STEP);
	}

	function zoomOut() {
		logScale = Math.max(LOG_MIN, logScale - ZOOM_STEP);
	}

	function handleZoomWidgetWheel(e: WheelEvent) {
		e.preventDefault();
		const delta = e.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP;
		logScale = Math.min(Math.max(LOG_MIN, logScale + delta), LOG_MAX);
	}

	function handleTracksWheel(e: WheelEvent) {
		if (e.ctrlKey || e.altKey) {
			e.preventDefault();
			const delta = e.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP;
			logScale = Math.min(Math.max(LOG_MIN, logScale + delta), LOG_MAX);
		}
	}

	$: zoomPercent = Math.round((logScale - LOG_MIN) / (LOG_MAX - LOG_MIN) * 100);

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
		const sampleRate = $preferences.audio.sampleRate;
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
			bitDepth: $preferences.project.exportBitDepth,
		});

		const filename = (filePath as string).split(/[\\/]/).pop() ?? 'mixdown.wav';
		notify(`Exporté : ${filename}`, 'success');
	}


</script>

<svelte:window on:keydown={handleKeyDown} />

{#if contextMenu}
	<TrackContextMenu
		x={contextMenu.x}
		y={contextMenu.y}
		trackId={contextMenu.trackId}
		trackName={contextMenu.trackName}
		on:addtrack={handleCreateTrack}
		on:rename={() => { if (contextMenu) renameTrackByIndex(contextMenu.trackIndex); }}
		on:delete={(e) => handleDeleteTrack(e.detail.trackId)}
		on:close={() => { contextMenu = null; }}
	/>
{/if}

{#if clipContextMenu}
	<ClipContextMenu
		x={clipContextMenu.x}
		y={clipContextMenu.y}
		selectionCount={$selection.size}
		canPaste={$clipboard.length > 0}
		on:cut={() => cutSelected()}
		on:copy={() => copySelected()}
		on:paste={() => pasteClips().catch(console.error)}
		on:delete={() => deleteSelected()}
		on:close={() => { clipContextMenu = null; }}
	/>
{/if}

<div class="container">
	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div class="zoom-ctrl" on:wheel={handleZoomWidgetWheel}>
		<button class="zoom-btn" on:click={zoomOut} title="Dézoomer">−</button>
		<input
			type="range"
			class="zoom-slider"
			bind:value={logScale}
			min={LOG_MIN}
			max={LOG_MAX}
			step={0.01}
		/>
		<button class="zoom-btn" on:click={zoomIn} title="Zoomer">+</button>
		<span class="zoom-value">{Math.round(scale)}</span>
	</div>

	<div class="left-panel">
		<div class="ruler-spacer"></div>
		<div class="instruments">
			{#each tracks as track, trackIndex}
				<!-- svelte-ignore a11y-no-static-element-interactions -->
				<!-- svelte-ignore a11y-no-static-element-interactions -->
				<div
					class="instrument"
					class:instrument--selected={selectedTrackIndex === trackIndex}
					style="--track-color: {TRACK_COLORS[trackIndex % TRACK_COLORS.length]}"
					on:mousedown={() => { selectedTrackIndex = trackIndex; }}
					on:contextmenu={(e) => handleTrackContextMenu(e, trackIndex)}
				>
					<!-- Row 1 — identity + routing -->
					<div class="track-head">
						<!-- svelte-ignore a11y-no-static-element-interactions -->
						<strong class="track-name" on:dblclick={(e) => makeEditable(e, trackIndex)}>{track.name}</strong>
						<button
							type="button"
							class="ms-btn"
							class:active={track.muted}
							title="Mute"
							on:click|stopPropagation={() => handleMuteToggle(trackIndex)}
						>M</button>
						<button
							type="button"
							class="ms-btn"
							class:active={track.solo}
							title="Solo"
							on:click|stopPropagation={() => handleSoloToggle(trackIndex)}
						>S</button>
					</div>

					<!-- Row 2 — mix controls -->
					<div class="track-controls">
						<!-- Pan knob + Volume fader/meter -->
						<div class="ctrl-row">
							<div class="ctrl-pan">
								<span class="ctrl-label">PAN</span>
								<Knob
									min={-100}
									max={100}
									defaultValue={0}
									bind:value={tracks[trackIndex].pan}
									on:release={() => handlePanRelease(trackIndex)}
								/>
							</div>
							<div class="ctrl-vol">
								<span class="ctrl-label">VOL</span>
								<FaderMeter
									bind:value={tracks[trackIndex].volume}
									peakL={($trackLevels[track.id]?.[0]) ?? 0}
									peakR={($trackLevels[track.id]?.[1]) ?? 0}
									defaultValue={75}
									on:release={() => handleVolumeRelease(trackIndex)}
								/>
							</div>
						</div>
					</div>
				</div>
			{/each}
		</div>
		<div class="left-panel-footer">
			<button type="button" class="add-track-btn" on:click={handleCreateTrack}>
				<span class="add-track-icon">+</span> Nouvelle piste
			</button>
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
			on:wheel={handleTracksWheel}
		>
			{#if tracks.length === 0}
				<div class="empty-tracks">
					<svg class="empty-tracks-icon" width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
						<path d="M9 18V5l12-2v13"/>
						<circle cx="6" cy="18" r="3"/>
						<circle cx="18" cy="16" r="3"/>
					</svg>
					<p class="empty-tracks-text">Aucune piste</p>
					<p class="empty-tracks-hint">Créez une piste ou glissez un fichier audio</p>
					<!-- svelte-ignore a11y-no-static-element-interactions a11y-click-events-have-key-events -->
					<button type="button" class="empty-tracks-cta" on:click={handleCreateTrack}>
						+ Nouvelle piste
					</button>
				</div>
			{/if}
			{#if fileDragPreview}
				<div
					class="clip-ghost"
					style="top: {fileDragPreview.trackIndex * 100}px; left: {fileDragPreview.startSec * scale}px; width: {fileDragPreview.durationSec != null ? fileDragPreview.durationSec * scale : 150}px;"
				></div>
			{/if}
			{#each tracks as { clips }, trackIndex}
				<Track {clips} {scale} {trackIndex} on:clipdragstart={onClipDragStart} on:clipcontextmenu={handleClipContextMenu} />
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

	// ── Zoom control widget ───────────────────────────────────────────────────
	.zoom-ctrl {
		position: absolute;
		right: 8px;
		bottom: 8px;
		z-index: 20;
		display: flex;
		align-items: center;
		gap: 4px;
		user-select: none;
		opacity: 0.4;
		transition: opacity 0.2s;

		&:hover {
			opacity: 1;
		}
	}

	.zoom-btn {
		background: none;
		border: none;
		color: rgba(247, 247, 249, 0.7);
		font-size: 13px;
		font-family: monospace;
		width: 14px;
		height: 14px;
		padding: 0;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		flex-shrink: 0;
		line-height: 1;
		transition: color 0.1s;

		&:hover { color: $text-color; }
		&:active { color: $accent-color; }
	}

	.zoom-slider {
		width: 90px;
		flex-shrink: 0;
		-webkit-appearance: none;
		appearance: none;
		height: 11px;
		background: transparent;
		outline: none;
		cursor: pointer;

		&::-webkit-slider-runnable-track {
			height: 2px;
			border-radius: 1px;
			background: rgba(255, 255, 255, 0.15);
		}

		&::-webkit-slider-thumb {
			-webkit-appearance: none;
			appearance: none;
			margin-top: -4px;
			width: 10px;
			height: 10px;
			border-radius: 50%;
			background: rgba(247, 247, 249, 0.6);
			cursor: pointer;
			transition: background 0.1s, transform 0.1s;

			&:hover {
				background: $accent-color;
				transform: scale(1.2);
			}
		}

		&::-moz-range-track {
			height: 2px;
			border-radius: 1px;
			background: rgba(255, 255, 255, 0.15);
		}

		&::-moz-range-thumb {
			width: 10px;
			height: 10px;
			border-radius: 50%;
			background: rgba(247, 247, 249, 0.6);
			border: none;
			cursor: pointer;
		}
	}

	.zoom-value {
		font-size: 9px;
		color: rgba(247, 247, 249, 0.4);
		font-family: 'SF Mono', 'Fira Code', ui-monospace, monospace;
		line-height: 1;
		min-width: 26px;
		text-align: right;
	}

	// ── Left panel ────────────────────────────────────────────────────────────
	.left-panel {
		width: 200px;
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		background-color: $pannel-background-color;
		border-right: 1px solid $border-color;
	}

	.ruler-spacer {
		height: 22px;
		flex-shrink: 0;
		background-color: $background-color;
		border-bottom: 1px solid $border-color;
	}

	.instruments {
		flex: 1;
		overflow-y: auto;
	}

	.left-panel-footer {
		flex-shrink: 0;
		padding: 8px;
		border-top: 1px solid $border-color;
	}

	.add-track-btn {
		width: 100%;
		height: $ctrl-height-sm;
		background: $surface-1;
		border: 1px dashed $border-subtle;
		border-radius: $radius-sm;
		color: $text-subtle;
		font-size: 11px;
		font-family: inherit;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 4px;
		transition: background 0.15s, color 0.15s, border-color 0.15s;

		&:hover {
			background: rgba(21, 113, 255, 0.1);
			border-color: rgba(21, 113, 255, 0.4);
			color: $text-color;
		}
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

	// ── Track instrument strip ───────────────────────────────────────────────────
	.instrument {
		height: 100px;
		display: flex;
		flex-direction: column;
		box-sizing: border-box;
		border-bottom: 1px solid $border-color;
		position: relative;
		transition: background 0.1s;

		// Color accent bar
		&::before {
			content: '';
			position: absolute;
			left: 0;
			top: 10px;
			bottom: 10px;
			width: 3px;
			border-radius: 0 2px 2px 0;
			background: var(--track-color, #{$accent-color});
		}

		&--selected { background: $surface-1; }
	}

	// Row 1 — name + M/S
	.track-head {
		height: 32px;
		flex-shrink: 0;
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 0 6px 0 13px;
		border-bottom: 1px solid rgba(255, 255, 255, 0.04);
	}

	.track-name {
		flex: 1;
		font-size: 11px;
		font-weight: 500;
		color: $text-color;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		min-width: 0;
		cursor: default;

		&:focus { outline: none; }
	}

	.ms-btn {
		width: 20px;
		height: 20px;
		flex-shrink: 0;
		padding: 0;
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: 3px;
		background: transparent;
		color: rgba($text-color, 0.45);
		font-size: 10px;
		font-weight: 700;
		line-height: 1;
		cursor: pointer;
		transition: background 0.1s, color 0.1s, border-color 0.1s;

		&:hover {
			color: $text-color;
			border-color: rgba($text-color, 0.35);
		}

		&.active {
			background: $accent-color;
			border-color: $accent-color;
			color: #fff;
		}
	}

	// Row 2 — mix controls (pan knob + vol fader/meter)
	.track-controls {
		flex: 1;
		display: flex;
		align-items: center;
		padding: 0 8px 4px 13px;
		gap: 0;
	}

	.ctrl-row {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.ctrl-label {
		font-size: 8px;
		font-family: $font-mono;
		color: $text-faint;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		line-height: 1;
		user-select: none;
	}

	// Pan: label above knob, fixed width
	.ctrl-pan {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 3px;
		flex-shrink: 0;
	}

	// Vol: label above fader, takes remaining space
	.ctrl-vol {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 5px;
		min-width: 0;
	}

	// ── Empty state ───────────────────────────────────────────────────────────
	.empty-tracks {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		pointer-events: none;

		.empty-tracks-icon {
			color: $text-faint;
			margin-bottom: 4px;
		}

		.empty-tracks-text {
			font-size: 13px;
			font-weight: 500;
			color: $text-muted;
			margin: 0;
		}

		.empty-tracks-hint {
			font-size: 11px;
			color: $text-faint;
			margin: 0;
		}

		.empty-tracks-cta {
			pointer-events: all;
			margin-top: 8px;
			height: $ctrl-height;
			padding: 0 18px;
			background: $accent-color;
			border: none;
			border-radius: $radius-md;
			color: #fff;
			font-size: 12px;
			font-weight: 600;
			font-family: inherit;
			cursor: pointer;
			transition: background 0.1s;

			&:hover { background: $accent-hover; }
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
