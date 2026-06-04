<script lang="ts">
	import Track from './Track.svelte';
	import TrackStrip from './TrackStrip.svelte';
	import TrackContextMenu from './TrackContextMenu.svelte';
	import ClipContextMenu from './ClipContextMenu.svelte';
	import type { Track as TrackType, Project } from '../types/global';
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { onMount, onDestroy, tick } from 'svelte';
	import { transport } from '../stores/transport';
	import { selection } from '../stores/selection';
	import { tempo } from '../stores/tempo';
	import { projectName, projectPath, midiGridBeats } from '../stores/project';
	import { pianoRollStore } from '../stores/pianoRoll';
	import { normalizeMidiGridBeats } from '../lib/midiGrid';
	import { projectEventStore } from '../shared/project/eventStore';
	import { addClip, moveClip, removeClip, updateTrack, muteTrack, soloTrack, seek, createTrack, deleteTrack, addMidiClip, moveMidiClip, removeMidiClip } from '../frontend/commands/dispatcher';
	import { clipboard } from '../stores/clipboard';
	import type { ClipboardEntry } from '../stores/clipboard';
	import { get } from 'svelte/store';
	import { snapPosition, getSnapSubdivisionSec } from '../lib/snapUtils';
	import { selectedTrack } from '../stores/selectedTrack';
	import { shouldHandleTimelineShortcuts } from '../frontend/shortcuts/context';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import {
		focusMidiClipForEdit,
		ensureClipPreview,
		syncPianoRollToSelection,
		trackColor,
	} from '../frontend/pianoRoll/clipEditor';
	import TimeRuler from './TimeRuler.svelte';
	import Playhead from './Playhead.svelte';
	import ZoomScrollbar from './ZoomScrollbar.svelte';
	import { startTransportScrub } from '../frontend/transportScrub';
	import {
		ARRANGE_POINTER_DRAG_THRESHOLD,
		ARRANGE_TRACK_HEIGHT,
		arrangeTrackIndexFromY,
		arrangeMinLogScale,
		arrangeVisibleMaxBars,
		clipsInArrangeSelbox,
		isNewArrangeTrackRow,
		trackIndexFromArrangeY,
	} from '../frontend/arrangeInteraction';
	import { ev, isEngineEvent, type TrackCreatedPayload } from '../shared/protocol/events';
	import type { CommandResult } from '../shared/protocol/commands';

	const TRACK_COLORS = [
		'#4d96ff', '#ff6b6b', '#6bcb77', '#ffd93d',
		'#c77dff', '#ff9a3c', '#00c9a7', '#ff6db6',
	];
	import { notify } from '../stores/status';
	import { trackLevels } from '../stores/trackLevels';

	// Live sync from the engine event store.
	// Local mutations (optimistic UI during drag/import) are reconciled by the
	// next store event — the reducer is idempotent so there's never a double-apply.
	let tracks: TrackType[] = [];
	const unsubStore = projectEventStore.subscribe((state) => {
		tracks = state.tracks;
		// Keep the tempo store in sync so MenuBar reflects undo/redo correctly.
		tempo.set({ bpm: state.bpm, numerator: state.numerator, denominator: state.denominator });
		const grid = normalizeMidiGridBeats(state.midi_grid_beats);
		midiGridBeats.set(grid);
		pianoRollStore.setQuantize(grid);
	});
	onDestroy(unsubStore);

	const unsubSelection = selection.subscribe((sel) => {
		syncPianoRollToSelection(tracks, sel);
	});
	onDestroy(unsubSelection);

	onDestroy(() => scrubCleanup?.());
	onDestroy(cleanupTracksPointer);
	onDestroy(() => instrumentsResizeObserver?.disconnect());
	onDestroy(() => tracksResizeObserver?.disconnect());
	onDestroy(() => {
		window.removeEventListener('mousemove', onResizeMove);
		window.removeEventListener('mouseup', onResizeEnd);
	});

	let contextMenu: { x: number; y: number; trackId: string; trackName: string; trackIndex: number } | null = null;
	let clipContextMenu: { x: number; y: number } | null = null;
	let selectedTrackIndex: number | null = null;

	function selectTrackIndex(trackIndex: number) {
		if (trackIndex < 0 || trackIndex >= tracks.length) return;
		selectedTrackIndex = trackIndex;
		keyboardFocus.setTimeline();
	}

	function selectTrackFromArrangeY(y: number) {
		const idx = trackIndexFromArrangeY(y, tracks.length);
		if (idx !== null) selectTrackIndex(idx);
	}

	function selectTrackFromCreateResult(result: CommandResult) {
		for (const raw of result.events) {
			if (!isEngineEvent(raw) || raw.type !== ev.TRACK_CREATED) continue;
			const id = (raw.payload as TrackCreatedPayload).id;
			const idx = tracks.findIndex((t) => t.id === id);
			if (idx >= 0) {
				selectTrackIndex(idx);
				return;
			}
		}
		if (tracks.length > 0) selectTrackIndex(tracks.length - 1);
	}

	function trackIdFromCreateResult(result: CommandResult): string | null {
		for (const raw of result.events) {
			if (!isEngineEvent(raw) || raw.type !== ev.TRACK_CREATED) continue;
			return (raw.payload as TrackCreatedPayload).id;
		}
		return null;
	}

	async function ensureTrackForClipPlacement(trackIndex: number): Promise<string | null> {
		if (!isNewArrangeTrackRow(trackIndex, tracks.length) && tracks[trackIndex]?.id) {
			return tracks[trackIndex].id;
		}
		try {
			const result = await createTrack(`Piste ${tracks.length + 1}`);
			selectTrackFromCreateResult(result);
			return trackIdFromCreateResult(result) ?? tracks[tracks.length - 1]?.id ?? null;
		} catch (e: unknown) {
			notify(`Erreur création piste : ${(e as Error)?.message ?? String(e)}`, 'error');
			return null;
		}
	}

	$: {
		if (selectedTrackIndex !== null && tracks[selectedTrackIndex]) {
			const t = tracks[selectedTrackIndex];
			selectedTrack.set({
				trackId: t.id,
				trackName: t.name,
				trackColor: TRACK_COLORS[selectedTrackIndex % TRACK_COLORS.length],
				instrument: t.instrument ?? null,
				effects: t.effects ?? [],
				modulators: t.modulators ?? [],
				mod_routes: t.mod_routes ?? [],
			});
		} else {
			selectedTrack.set({ trackId: null, trackName: '', trackColor: '#4d96ff', instrument: null, effects: [], modulators: [], mod_routes: [] });
		}
	}

	let logScale: number = Math.log(80);
	let pendingArrangeFitZoom = false;
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
		selectedTrackIndex = null;
		transport.update((t) => ({ ...t, isPlaying: false, position: 0 }));

		const { applyProjectUiLayout } = await import('../lib/projectUiLayout');
		applyProjectUiLayout(dto.ui_layout);

		projectName.set(dto.name);
		tempo.set({ bpm: dto.bpm, numerator: dto.numerator, denominator: dto.denominator });
		const grid = normalizeMidiGridBeats(dto.midi_grid_beats ?? 0.25);
		midiGridBeats.set(grid);
		pianoRollStore.setQuantize(grid);

		// Seed the engine store — the subscription above will update `tracks` reactively,
		// which in turn triggers the $: buffer-preparation block for all clips.
		projectEventStore.seedFromSnapshot(dto);

		pendingArrangeFitZoom = true;
		await tick();
		requestAnimationFrame(() => tryPendingArrangeFitZoom());

		// Window title — set from frontend so the window is guaranteed to exist
		getCurrentWindow().setTitle(`${dto.name} — Echoes Studio`).catch(console.error);
	}

	// ── Playhead ──────────────────────────────────────────────────────────────
	// Position is pushed by the CPAL engine via POSITION_UPDATED events (~30 fps).
	// The animFrame loop is kept for future per-frame UI work (e.g. VU meters).
	let animFrameId: number;

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
				const x = winX - rect.left + tracksEl.scrollLeft;
				const y = winY - rect.top + tracksEl.scrollTop;
				if (x < 0 || x > rect.width || y < 0) { fileDragPreview = null; return; }
				const trackIndex = arrangeTrackIndexFromY(y, tracks.length);
				fileDragPreview = { trackIndex, startSec: Math.max(0, x / scale), durationSec: pendingDragDuration ?? fileDragPreview?.durationSec ?? null };
			}

			const AUDIO_EXTS = new Set(['wav', 'mp3', 'flac', 'ogg', 'oga']);
			const isAudioFile = (p: string) =>
				AUDIO_EXTS.has(p.split('.').pop()?.toLowerCase() ?? '');

			type DragPayload = { paths: string[]; position: { x: number; y: number } };
			const dragFns = await Promise.all([
				listen<DragPayload>('tauri://drag-enter', ({ payload }) => {
					pendingDragDuration = null;
					const path = payload.paths[0];
					if (!path || !isAudioFile(path)) return;
					updateDragPreview(payload.position.x, payload.position.y);
					invoke<number>('get_audio_clip_duration', { filePath: path })
						.then((dur) => {
							pendingDragDuration = dur;
							if (fileDragPreview) fileDragPreview = { ...fileDragPreview, durationSec: dur };
						})
						.catch(() => {});
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
					if (!isAudioFile(filePath)) {
						notify(`Format non supporté. Utilisez WAV, MP3, FLAC ou OGG.`, 'error');
						fileDragPreview = null;
						return;
					}

					const trackIndex = preview?.trackIndex ?? 0;
					const startSec = preview?.startSec ?? 0;

					const doImport = (dur: number) => {
						ensureTrackForClipPlacement(trackIndex)
							.then((trackId) => {
								if (!trackId) { fileDragPreview = null; return; }
								return addClip(trackId, filePath, startSec, startSec + dur);
							})
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
						if (selectedTrackIndex !== null && tracks[selectedTrackIndex]) {
							handleDeleteTrack(tracks[selectedTrackIndex].id);
						}
					}
				}),
				listen<string>('audio-file-picked', async ({ payload: filePath }) => {
					try {
						const dur = await invoke<number>('get_audio_clip_duration', { filePath });
						const startSec = $transport.position;
						const trackIndex =
							selectedTrackIndex !== null && tracks[selectedTrackIndex]
								? selectedTrackIndex
								: tracks.length;
						const trackId = await ensureTrackForClipPlacement(trackIndex);
						if (!trackId) return;
						await addClip(trackId, filePath, startSec, startSec + dur);
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

	$: pxPerBeat = (60 / $tempo.bpm) * scale;
	$: arrangeMaxBars = arrangeVisibleMaxBars(tracks, $tempo.bpm, $tempo.numerator);
	$: arrangeTotalBeats = arrangeMaxBars * $tempo.numerator;
	$: arrangeContentWidthPx = arrangeTotalBeats * pxPerBeat;
	$: playheadX = $transport.position * scale;
	$: playheadMarkerX = playheadX - tracksScrollLeft;

	$: if (instrumentsEl) {
		updateInstrumentsOverflow();
		instrumentsResizeObserver?.disconnect();
		if (typeof ResizeObserver !== 'undefined') {
			instrumentsResizeObserver = new ResizeObserver(updateInstrumentsOverflow);
			instrumentsResizeObserver.observe(instrumentsEl);
		}
	}

	$: tracks.length, instrumentsEl, void tick().then(updateInstrumentsOverflow);

	$: if (tracksEl && !tracksResizeObserver) {
		updateArrangeScrollbars();
		if (typeof ResizeObserver !== 'undefined') {
			tracksResizeObserver = new ResizeObserver(() => {
				updateArrangeScrollbars();
				tryPendingArrangeFitZoom();
			});
			tracksResizeObserver.observe(tracksEl);
		}
	}

	$: if (pendingArrangeFitZoom && tracksEl) {
		void tick().then(tryPendingArrangeFitZoom);
	}

	$: scale, arrangeContentWidthPx, arrangeMaxBars, rulerViewportWidth, tracksEl, void tick().then(() => {
		if (!tracksEl) return;
		enforceArrangeMinZoom();
		const maxScroll = Math.max(0, tracksEl.scrollWidth - tracksEl.clientWidth);
		if (tracksEl.scrollLeft > maxScroll) {
			tracksEl.scrollLeft = maxScroll;
			tracksScrollLeft = maxScroll;
		}
		updateArrangeScrollbars();
	});

	let tracksEl: HTMLElement;
	let instrumentsEl: HTMLDivElement | undefined;
	let instrumentsOverflow = false;
	let instrumentsResizeObserver: ResizeObserver | undefined;
	let arrangeRulerEl: HTMLDivElement;

	function updateInstrumentsOverflow() {
		if (!instrumentsEl) {
			instrumentsOverflow = false;
			return;
		}
		instrumentsOverflow = instrumentsEl.scrollHeight > instrumentsEl.clientHeight + 1;
	}
	let dragging: { origTrackIndex: number; trackIndex: number; clipIndex: number; offsetPx: number } | null = null;
	let midiDragging: { origTrackIndex: number; trackIndex: number; clipId: string; offsetPx: number; origStart: number; origEnd: number } | null = null;
	let resizing:
		| {
				kind: 'audio' | 'midi';
				edge: 'left' | 'right';
				trackIndex: number;
				clipIndex?: number;
				clipId: string;
				origStart: number;
				origEnd: number;
				origOffset: number;
				maxSourceDuration: number;
		  }
		| null = null;
	let midiClipContextTarget: string | null = null;
	let fileDragPreview: { trackIndex: number; startSec: number; durationSec: number | null } | null = null;
	let pendingDragDuration: number | null = null;

	let scrubCleanup: (() => void) | null = null;

	type TracksPointerState =
		| { kind: 'none' }
		| {
				kind: 'pending';
				startX: number;
				startY: number;
				clientX: number;
				clientY: number;
				additive: boolean;
		  }
		| { kind: 'selbox'; x0: number; y0: number; x1: number; y1: number; additive: boolean };

	let tracksPointer: TracksPointerState = { kind: 'none' };
	let tracksScrollTop = 0;
	let tracksScrollLeft = 0;
	let rulerViewportWidth = 0;
	let hScrollStart = 0;
	let hScrollEnd = 1;
	let tracksResizeObserver: ResizeObserver | undefined;

	function arrangeContentX(clientX: number, originEl: HTMLElement): number {
		const rect = originEl.getBoundingClientRect();
		const scroll = originEl === tracksEl ? tracksEl.scrollLeft : tracksScrollLeft;
		return clientX - rect.left + scroll;
	}

	function tracksContentCoords(event: MouseEvent): { x: number; y: number } {
		if (!tracksEl) return { x: 0, y: 0 };
		const rect = tracksEl.getBoundingClientRect();
		return {
			x: event.clientX - rect.left + tracksEl.scrollLeft,
			y: event.clientY - rect.top + tracksEl.scrollTop,
		};
	}

	function positionFromClientX(clientX: number, altKey: boolean): number {
		if (!tracksEl) return 0;
		return snap(Math.max(0, arrangeContentX(clientX, tracksEl) / scale), altKey);
	}

	function updateArrangeScrollbars() {
		if (!tracksEl) return;
		const totalW = tracksEl.scrollWidth;
		const vpW = tracksEl.clientWidth;
		rulerViewportWidth = vpW;
		hScrollStart = totalW > vpW ? tracksScrollLeft / totalW : 0;
		hScrollEnd = totalW > vpW ? Math.min(1, (tracksScrollLeft + vpW) / totalW) : 1;
	}

	function onTracksScroll() {
		if (!tracksEl) return;
		tracksScrollTop = tracksEl.scrollTop;
		tracksScrollLeft = tracksEl.scrollLeft;
		updateArrangeScrollbars();
	}

	function arrangeLogScaleFloor(): number {
		if (!tracksEl || tracksEl.clientWidth <= 0) return Math.log(10);
		return arrangeMinLogScale(
			tracksEl.clientWidth,
			arrangeMaxBars,
			$tempo.numerator,
			$tempo.bpm,
		);
	}

	function clampArrangeLogScale(value: number): number {
		return Math.min(LOG_MAX, Math.max(arrangeLogScaleFloor(), value));
	}

	function applyArrangeMaxDezoom(): boolean {
		if (!tracksEl || tracksEl.clientWidth <= 0) return false;
		logScale = arrangeLogScaleFloor();
		tracksEl.scrollLeft = 0;
		tracksScrollLeft = 0;
		hScrollStart = 0;
		hScrollEnd = 1;
		updateArrangeScrollbars();
		return true;
	}

	function tryPendingArrangeFitZoom() {
		if (!pendingArrangeFitZoom) return;
		if (applyArrangeMaxDezoom()) pendingArrangeFitZoom = false;
	}

	function enforceArrangeMinZoom() {
		if (!tracksEl) return;
		const floor = arrangeLogScaleFloor();
		if (logScale >= floor) return;
		logScale = floor;
		tracksEl.scrollLeft = 0;
		tracksScrollLeft = 0;
	}

	function onArrangeHScrollChange(e: CustomEvent<{ start: number; end: number; cause: string }>) {
		if (!tracksEl) return;
		const { start: ns, end: ne, cause } = e.detail;
		const totalBeats = arrangeTotalBeats;
		const vpW = tracksEl.clientWidth;
		const bpm = $tempo.bpm;
		const leftBeat = ns * totalBeats;
		const rightBeat = ne * totalBeats;
		const spanBeats = rightBeat - leftBeat;
		if (spanBeats <= 0 || vpW <= 0) return;

		const clampedLog = clampArrangeLogScale(Math.log((vpW / spanBeats) * bpm / 60));
		const clampedPxPerBeat = (Math.exp(clampedLog) * 60) / bpm;
		const clampedSpanBeats = vpW / clampedPxPerBeat;
		const atMinZoom = clampedLog <= arrangeLogScaleFloor() + 1e-9;

		let clampedLeftBeat: number;
		let clampedRightBeat: number;

		if (atMinZoom) {
			clampedLeftBeat = 0;
			clampedRightBeat = totalBeats;
		} else if (cause === 'resize-start') {
			clampedRightBeat = Math.min(totalBeats, rightBeat);
			clampedLeftBeat = Math.max(0, clampedRightBeat - clampedSpanBeats);
		} else if (cause === 'resize-end') {
			clampedLeftBeat = Math.max(0, leftBeat);
			clampedRightBeat = Math.min(totalBeats, clampedLeftBeat + clampedSpanBeats);
		} else {
			const center = (leftBeat + rightBeat) * 0.5;
			clampedLeftBeat = Math.max(
				0,
				Math.min(center - clampedSpanBeats * 0.5, totalBeats - clampedSpanBeats),
			);
			clampedRightBeat = clampedLeftBeat + clampedSpanBeats;
		}

		const newScrollLeft = atMinZoom ? 0 : clampedLeftBeat * clampedPxPerBeat;
		hScrollStart = clampedLeftBeat / totalBeats;
		hScrollEnd = clampedRightBeat / totalBeats;

		if (Math.abs(clampedLog - logScale) < 0.001) {
			tracksEl.scrollLeft = newScrollLeft;
			tracksScrollLeft = newScrollLeft;
			updateArrangeScrollbars();
		} else {
			logScale = clampedLog;
			tick().then(() => {
				if (!tracksEl) return;
				tracksEl.scrollLeft = newScrollLeft;
				tracksScrollLeft = newScrollLeft;
				updateArrangeScrollbars();
			});
		}
	}

	function seekToClientX(clientX: number, altKey: boolean) {
		const positionSec = positionFromClientX(clientX, altKey);
		transport.update((t) => ({ ...t, position: positionSec }));
		seek(positionSec).catch(console.error);
	}

	function cleanupTracksPointer() {
		window.removeEventListener('mousemove', onTracksPointerMove);
		window.removeEventListener('mouseup', onTracksPointerUp);
		tracksPointer = { kind: 'none' };
	}

	function onTracksPointerMove(event: MouseEvent) {
		if (tracksPointer.kind === 'none') return;

		if (tracksPointer.kind === 'pending') {
			const dx = event.clientX - tracksPointer.clientX;
			const dy = event.clientY - tracksPointer.clientY;
			if (Math.hypot(dx, dy) < ARRANGE_POINTER_DRAG_THRESHOLD) return;

			const { x, y } = tracksContentCoords(event);
			tracksPointer = {
				kind: 'selbox',
				x0: tracksPointer.startX,
				y0: tracksPointer.startY,
				x1: x,
				y1: y,
				additive: tracksPointer.additive,
			};
			return;
		}

		const { x, y } = tracksContentCoords(event);
		tracksPointer = { ...tracksPointer, x1: x, y1: y };
	}

	function onTracksPointerUp(event: MouseEvent) {
		if (tracksPointer.kind === 'pending') {
			selectTrackFromArrangeY(tracksPointer.startY);
			seekToClientX(event.clientX, event.altKey);
		} else if (tracksPointer.kind === 'selbox') {
			const ids = clipsInArrangeSelbox(tracks, tracksPointer, scale);
			if (ids.length === 1 && !tracksPointer.additive) {
				const clipId = ids[0]!;
				selection.setSingle(clipId);
				activateArrangeClip(clipId);
			} else if (ids.length > 0) {
				selection.setMany(ids, { additive: tracksPointer.additive });
				selectTrackFromArrangeY(tracksPointer.y0);
			} else if (!tracksPointer.additive) {
				selection.clear();
				selectTrackFromArrangeY(tracksPointer.y0);
			}
		}

		cleanupTracksPointer();
	}

	function activateArrangeClip(clipId: string) {
		for (let trackIndex = 0; trackIndex < tracks.length; trackIndex++) {
			const track = tracks[trackIndex];
			const midiClip = track.midi_clips?.find((c) => c.id === clipId);
			if (midiClip) {
				selectTrackIndex(trackIndex);
				ensureClipPreview(track, midiClip, trackColor(trackIndex));
				return;
			}
			if (track.clips.some((c) => c.id === clipId)) {
				selectTrackIndex(trackIndex);
				return;
			}
		}
	}

	function snap(
		positionSec: number,
		altKey: boolean,
		excludeClip?: { trackIndex: number; clipIndex: number }
	): number {
		return snapPosition(positionSec, altKey, $tempo, scale, $midiGridBeats, tracks, excludeClip);
	}

	function beginScrub(event: MouseEvent, originEl: HTMLElement) {
		scrubCleanup?.();
		scrubCleanup = startTransportScrub(
			event,
			(clientX, altKey) => snap(Math.max(0, arrangeContentX(clientX, originEl) / scale), altKey),
			(positionSec) => {
				scrubCleanup = null;
				seek(positionSec).catch(console.error);
			},
		);
	}

	function handleRulerMouseDown(event: CustomEvent<MouseEvent>) {
		if (get(keyboardFocus) !== 'piano-roll') keyboardFocus.setTimeline();
		beginScrub(event.detail, arrangeRulerEl);
	}

	function handleTracksMouseDown(event: MouseEvent) {
		if (dragging || midiDragging || !tracksEl) return;
		if (event.button !== 0) return;

		if (get(keyboardFocus) === 'piano-roll') {
			// En mode édition piano-roll, ne pas casser le focus ni la sélection
			// pour un clic dans la zone tracks. Seul le scrub Alt+drag reste possible.
			if (event.altKey) beginScrub(event, tracksEl);
			return;
		}

		keyboardFocus.setTimeline();

		if (event.altKey) {
			beginScrub(event, tracksEl);
			return;
		}

		const { x, y } = tracksContentCoords(event);
		const additive = event.ctrlKey || event.metaKey;
		if (!additive) selection.clear();

		tracksPointer = {
			kind: 'pending',
			startX: x,
			startY: y,
			clientX: event.clientX,
			clientY: event.clientY,
			additive,
		};

		window.addEventListener('mousemove', onTracksPointerMove);
		window.addEventListener('mouseup', onTracksPointerUp);
	}

	function deleteSelected() {
		// Optimistic: remove from local state immediately for instant feedback.
		tracks.forEach((track) => {
			const toDeleteAudio = track.clips.filter((c) => $selection.ids.has(c.id));
			for (const clip of toDeleteAudio) {
				removeClip(track.id, clip.id).catch(console.error);
			}
			const toDeleteMidi = (track.midi_clips ?? []).filter((c) => $selection.ids.has(c.id));
			for (const clip of toDeleteMidi) {
				removeMidiClip(track.id, clip.id).catch(console.error);
			}
		});
		tracks = tracks.map((track) => ({
			...track,
			clips: track.clips.filter((c) => !$selection.ids.has(c.id)),
			midi_clips: (track.midi_clips ?? []).filter((c) => !$selection.ids.has(c.id)),
		}));
		midiClipContextTarget = null;
		selection.clear();
	}

	function copySelected() {
		const sel = $selection.ids;
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
		keyboardFocus.setTimeline();
		const { id, trackIndex, x, y } = e.detail;
		selectTrackIndex(trackIndex);
		// If the right-clicked clip is not in the selection, make it the sole selection.
		if (!$selection.ids.has(id)) {
			selection.setSingle(id);
		}
		contextMenu = null;
		clipContextMenu = { x, y };
	}

	function handleKeyDown(e: KeyboardEvent) {
		if (!shouldHandleTimelineShortcuts(e)) return;

		const ctrl = e.ctrlKey || e.metaKey;

		if ((e.key === 'Delete' || e.key === 'Backspace') && !ctrl) {
			e.preventDefault();
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

	function onMidiClipDragStart(e: CustomEvent<{ offsetPx: number; trackIndex: number; clipId: string }>) {
		const { trackIndex, clipId, offsetPx } = e.detail;
		selectTrackIndex(trackIndex);
		const track = tracks[trackIndex];
		const clip = track?.midi_clips?.find((c) => c.id === clipId);
		if (!clip) return;
		midiDragging = { origTrackIndex: trackIndex, trackIndex, clipId, offsetPx, origStart: clip.start, origEnd: clip.end };
		window.addEventListener('mousemove', onMidiDragMove);
		window.addEventListener('mouseup', onMidiDragEnd);
	}

	function onMidiDragMove(e: MouseEvent) {
		if (!midiDragging || !tracksEl) return;
		const rect = tracksEl.getBoundingClientRect();
		const { clipId, offsetPx, origEnd, origStart } = midiDragging;
		const rawStart = Math.max(0, (e.clientX - rect.left + tracksEl.scrollLeft - offsetPx) / scale);
		const newStart = snap(rawStart, e.altKey);
		const duration = origEnd - origStart;

		const y = e.clientY - rect.top + tracksEl.scrollTop;
		const newTrackIndex = Math.max(0, Math.min(tracks.length - 1, Math.floor(y / ARRANGE_TRACK_HEIGHT)));
		const prevTrackIndex = midiDragging.trackIndex;

		if (newTrackIndex !== prevTrackIndex) {
			const prevTrack = tracks[prevTrackIndex];
			const clip = prevTrack?.midi_clips?.find((c) => c.id === clipId);
			if (!clip) return;
			const updatedClip = { ...clip, start: newStart, end: newStart + duration };
			tracks[prevTrackIndex] = {
				...prevTrack,
				midi_clips: prevTrack.midi_clips.filter((c) => c.id !== clipId),
			};
			tracks[newTrackIndex] = {
				...tracks[newTrackIndex],
				midi_clips: [...tracks[newTrackIndex].midi_clips, updatedClip],
			};
			midiDragging = { ...midiDragging, trackIndex: newTrackIndex };
		} else {
			const track = tracks[newTrackIndex];
			if (!track) return;
			tracks[newTrackIndex] = {
				...track,
				midi_clips: track.midi_clips.map((c) =>
					c.id === clipId ? { ...c, start: newStart, end: newStart + duration } : c
				),
			};
		}
		tracks = tracks;
	}

	function onMidiDragEnd() {
		if (midiDragging) {
			const { origTrackIndex, trackIndex, clipId } = midiDragging;
			const srcTrack = tracks[origTrackIndex];
			const dstTrack = tracks[trackIndex];
			const clip = dstTrack?.midi_clips?.find((c) => c.id === clipId);
			if (dstTrack && clip) {
				const targetTrackId = origTrackIndex !== trackIndex ? dstTrack.id : undefined;
				moveMidiClip(srcTrack?.id ?? dstTrack.id, clipId, clip.start, clip.end, clip.offset, targetTrackId).catch(console.error);
			}
		}
		midiDragging = null;
		window.removeEventListener('mousemove', onMidiDragMove);
		window.removeEventListener('mouseup', onMidiDragEnd);
	}

	function onMidiClipActivate(e: CustomEvent<{ id: string; trackIndex: number }>) {
		const { id, trackIndex } = e.detail;
		const track = tracks[trackIndex];
		if (!track) return;
		const clip = track.midi_clips?.find((c) => c.id === id);
		if (!clip) return;
		ensureClipPreview(track, clip, trackColor(trackIndex));
	}

	function onMidiClipDblClick(e: CustomEvent<{ id: string; trackIndex: number }>) {
		const { id, trackIndex } = e.detail;
		const track = tracks[trackIndex];
		if (!track) return;
		const clip = track.midi_clips?.find((c) => c.id === id);
		if (!clip) return;
		focusMidiClipForEdit(track, clip, trackColor(trackIndex));
	}

	function onMidiClipContextMenu(e: CustomEvent<{ id: string; trackIndex: number; x: number; y: number }>) {
		keyboardFocus.setTimeline();
		const { id, trackIndex, x, y } = e.detail;
		selectTrackIndex(trackIndex);
		// Reuse clip context menu with a remove action handled via selection
		selection.setSingle(id);
		midiClipContextTarget = id;
		contextMenu = null;
		clipContextMenu = { x, y };
	}

	function handleTrackDoubleClick(e: CustomEvent<{ offsetPx: number; trackIndex: number }>) {
		const { offsetPx, trackIndex } = e.detail;
		const track = tracks[trackIndex];
		if (!track) return;

		selectTrackIndex(trackIndex);

		const startSec = Math.max(0, offsetPx / scale);
		const bpm = $tempo.bpm;
		const fourBeats = (4 / bpm) * 60;
		addMidiClip(track.id, startSec, startSec + fourBeats).catch(console.error);
	}

	function onClipDragStart(e: CustomEvent<{ offsetPx: number; trackIndex: number; clipIndex: number }>) {
		selectTrackIndex(e.detail.trackIndex);
		dragging = { origTrackIndex: e.detail.trackIndex, ...e.detail };
		window.addEventListener('mousemove', onDragMove);
		window.addEventListener('mouseup', onDragEnd);
	}

	function onDragMove(e: MouseEvent) {
		if (!dragging || !tracksEl) return;
		const rect = tracksEl.getBoundingClientRect();
		const { clipIndex, offsetPx } = dragging;
		const rawStart = Math.max(0, (e.clientX - rect.left + tracksEl.scrollLeft - offsetPx) / scale);

		const y = e.clientY - rect.top + tracksEl.scrollTop;
		const newTrackIndex = Math.max(0, Math.min(tracks.length - 1, Math.floor(y / ARRANGE_TRACK_HEIGHT)));
		const prevTrackIndex = dragging.trackIndex;

		if (newTrackIndex !== prevTrackIndex) {
			const clip = tracks[prevTrackIndex]?.clips[clipIndex];
			if (!clip) return;
			const newStart = snap(rawStart, e.altKey);
			const duration = clip.end - clip.start;
			const updatedClip = { ...clip, start: newStart, end: newStart + duration };
			const prevClips = [...tracks[prevTrackIndex].clips];
			prevClips.splice(clipIndex, 1);
			tracks[prevTrackIndex] = { ...tracks[prevTrackIndex], clips: prevClips };
			const newClips = [...tracks[newTrackIndex].clips, updatedClip];
			tracks[newTrackIndex] = { ...tracks[newTrackIndex], clips: newClips };
			dragging = { ...dragging, trackIndex: newTrackIndex, clipIndex: newClips.length - 1 };
		} else {
			const newStart = snap(rawStart, e.altKey, { trackIndex: newTrackIndex, clipIndex });
			const clip = tracks[newTrackIndex]?.clips[clipIndex];
			if (!clip) return;
			const duration = clip.end - clip.start;
			tracks[newTrackIndex].clips[clipIndex] = { ...clip, start: newStart, end: newStart + duration };
		}
		tracks = tracks;
	}

	function onDragEnd() {
		if (dragging) {
			const { origTrackIndex, trackIndex, clipIndex } = dragging;
			const srcTrack = tracks[origTrackIndex];
			const dstTrack = tracks[trackIndex];
			const clip = dstTrack?.clips[clipIndex];
			if (dstTrack && clip) {
				const targetTrackId = origTrackIndex !== trackIndex ? dstTrack.id : undefined;
				moveClip(srcTrack?.id ?? dstTrack.id, clip.id, clip.start, clip.end, clip.offset, targetTrackId).catch(console.error);
			}
		}
		dragging = null;
		window.removeEventListener('mousemove', onDragMove);
		window.removeEventListener('mouseup', onDragEnd);
	}

	// ── Clip resize (trim) ──────────────────────────────────────────────────────
	// Left edge: hide leading content (content origin `start - offset` stays put);
	// right edge: trim/extend the tail. Audio clamps the tail to the source length.
	const MIN_CLIP_PX = 8;

	function onClipResizeStart(
		e: CustomEvent<{ edge: 'left' | 'right'; clientX: number; trackIndex: number; clipIndex: number }>,
	) {
		const { edge, trackIndex, clipIndex } = e.detail;
		selectTrackIndex(trackIndex);
		const clip = tracks[trackIndex]?.clips[clipIndex];
		if (!clip) return;
		const sourceDuration = (clip.peaks_64.length * 64) / clip.sample_rate;
		resizing = {
			kind: 'audio',
			edge,
			trackIndex,
			clipIndex,
			clipId: clip.id,
			origStart: clip.start,
			origEnd: clip.end,
			origOffset: clip.offset,
			maxSourceDuration: sourceDuration,
		};
		window.addEventListener('mousemove', onResizeMove);
		window.addEventListener('mouseup', onResizeEnd);
	}

	function onMidiClipResizeStart(
		e: CustomEvent<{ edge: 'left' | 'right'; clientX: number; trackIndex: number; clipId: string }>,
	) {
		const { edge, trackIndex, clipId } = e.detail;
		selectTrackIndex(trackIndex);
		const clip = tracks[trackIndex]?.midi_clips?.find((c) => c.id === clipId);
		if (!clip) return;
		resizing = {
			kind: 'midi',
			edge,
			trackIndex,
			clipId,
			origStart: clip.start,
			origEnd: clip.end,
			origOffset: clip.offset,
			maxSourceDuration: Infinity,
		};
		window.addEventListener('mousemove', onResizeMove);
		window.addEventListener('mouseup', onResizeEnd);
	}

	function onResizeMove(e: MouseEvent) {
		if (!resizing || !tracksEl) return;
		const { edge, trackIndex, origStart, origEnd, origOffset, maxSourceDuration } = resizing;
		const minWidth = MIN_CLIP_PX / scale;
		const origin = origStart - origOffset;

		const excludeClip =
			resizing.kind === 'audio' ? { trackIndex, clipIndex: resizing.clipIndex! } : undefined;
		const posSec = snap(Math.max(0, arrangeContentX(e.clientX, tracksEl) / scale), e.altKey, excludeClip);

		let newStart = origStart;
		let newEnd = origEnd;
		let newOffset = origOffset;

		if (edge === 'left') {
			const minStart = Math.max(0, origin);
			newStart = Math.min(Math.max(posSec, minStart), origEnd - minWidth);
			newOffset = newStart - origin;
		} else {
			const maxEnd = resizing.kind === 'audio' ? origin + maxSourceDuration : Infinity;
			newEnd = Math.min(Math.max(posSec, origStart + minWidth), maxEnd);
		}

		applyResizeOptimistic(newStart, newEnd, newOffset);
	}

	function applyResizeOptimistic(start: number, end: number, offset: number) {
		if (!resizing) return;
		const { trackIndex } = resizing;
		const track = tracks[trackIndex];
		if (!track) return;
		if (resizing.kind === 'audio') {
			const { clipIndex } = resizing;
			const clip = track.clips[clipIndex!];
			if (!clip) return;
			track.clips[clipIndex!] = { ...clip, start, end, offset };
		} else {
			track.midi_clips = track.midi_clips.map((c) =>
				c.id === resizing!.clipId ? { ...c, start, end, offset } : c,
			);
		}
		tracks = tracks;
	}

	function onResizeEnd() {
		if (resizing) {
			const { trackIndex } = resizing;
			const track = tracks[trackIndex];
			if (track) {
				if (resizing.kind === 'audio') {
					const clip = track.clips[resizing.clipIndex!];
					if (clip) moveClip(track.id, clip.id, clip.start, clip.end, clip.offset).catch(console.error);
				} else {
					const clip = track.midi_clips?.find((c) => c.id === resizing!.clipId);
					if (clip) moveMidiClip(track.id, clip.id, clip.start, clip.end, clip.offset).catch(console.error);
				}
			}
		}
		resizing = null;
		window.removeEventListener('mousemove', onResizeMove);
		window.removeEventListener('mouseup', onResizeEnd);
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

	async function handleCreateTrack() {
		try {
			const result = await createTrack(`Piste ${tracks.length + 1}`);
			selectTrackFromCreateResult(result);
		} catch (e: unknown) {
			notify(`Erreur création piste : ${(e as Error)?.message ?? String(e)}`, 'error');
		}
	}

	function handleDeleteTrack(trackId: string) {
		deleteTrack(trackId).catch((e: unknown) =>
			notify(`Erreur suppression piste : ${(e as Error)?.message ?? String(e)}`, 'error')
		);
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

	// ── Zoom controls ─────────────────────────────────────────────────────────
	const LOG_MAX = Math.log(2000);
	const ZOOM_STEP = 0.12;

	function handleTracksWheel(e: WheelEvent) {
		if (e.ctrlKey || e.altKey) {
			e.preventDefault();
			const delta = e.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP;
			const next = clampArrangeLogScale(logScale + delta);
			if (next <= arrangeLogScaleFloor() + 1e-9 && tracksEl) {
				tracksEl.scrollLeft = 0;
				tracksScrollLeft = 0;
			}
			logScale = next;
			updateArrangeScrollbars();
		}
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
		selectionCount={$selection.ids.size}
		canPaste={$clipboard.length > 0}
		on:cut={() => cutSelected()}
		on:copy={() => copySelected()}
		on:paste={() => pasteClips().catch(console.error)}
		on:delete={() => deleteSelected()}
		on:close={() => { clipContextMenu = null; }}
	/>
{/if}

<div class="container">
	<div class="left-panel">
		<div class="ruler-spacer"></div>
		<div class="instruments" bind:this={instrumentsEl}>
			{#each tracks as track, trackIndex}
				<TrackStrip
					{track}
					selected={selectedTrackIndex === trackIndex}
					color={TRACK_COLORS[trackIndex % TRACK_COLORS.length]}
					bind:volume={tracks[trackIndex].volume}
					bind:pan={tracks[trackIndex].pan}
					peakL={($trackLevels[track.id]?.[0]) ?? 0}
					peakR={($trackLevels[track.id]?.[1]) ?? 0}
					on:select={() => selectTrackIndex(trackIndex)}
					on:contextmenu={(e) => { contextMenu = { x: e.detail.clientX, y: e.detail.clientY, trackId: track.id, trackName: track.name, trackIndex }; }}
					on:mutetoggle={() => handleMuteToggle(trackIndex)}
					on:solotoggle={() => handleSoloToggle(trackIndex)}
					on:volumerelease={() => handleVolumeRelease(trackIndex)}
					on:panrelease={() => handlePanRelease(trackIndex)}
					on:rename={(e) => { tracks[trackIndex] = { ...tracks[trackIndex], name: e.detail.name }; tracks = tracks; updateTrack(tracks[trackIndex].id, { name: e.detail.name, volume: tracks[trackIndex].volume }).catch(console.error); }}
				on:openpianoroll={() => {
					const firstClip = track.midi_clips?.[0];
					if (firstClip) {
						selection.setSingle(firstClip.id);
						focusMidiClipForEdit(track, firstClip, trackColor(trackIndex));
					} else {
						const bpm = $tempo.bpm;
						const fourBeats = (4 / bpm) * 60;
						addMidiClip(track.id, 0, fourBeats).catch(console.error);
					}
				}}
				/>
			{/each}
		</div>
		<div class="left-panel-footer" class:has-scroll-separator={instrumentsOverflow}>
			<button type="button" class="add-track-btn" on:click={handleCreateTrack}>
				<span class="add-track-icon">+</span> Nouvelle piste
			</button>
		</div>
	</div>

	<div class="right-panel">
		<!-- svelte-ignore a11y-no-static-element-interactions a11y-click-events-have-key-events -->
		<TimeRuler
			bind:rulerEl={arrangeRulerEl}
			pxPerBeat={pxPerBeat}
			numerator={$tempo.numerator}
			scrollPx={tracksScrollLeft}
			viewportWidthPx={rulerViewportWidth}
			maxBars={arrangeMaxBars}
			on:mousedown={handleRulerMouseDown}
		/>

		<Playhead x={playheadMarkerX} />

		<div class="tracks-column">
			<!-- svelte-ignore a11y-no-static-element-interactions a11y-click-events-have-key-events -->
			<div
				class="tracks"
				class:is-dragging={dragging || midiDragging}
				class:is-resizing={resizing}
				class:is-file-dragging={fileDragPreview}
				class:is-selbox-dragging={tracksPointer.kind === 'selbox'}
				bind:this={tracksEl}
				on:drop={handleDrop}
				on:dragover={handleDragOver}
				on:dragenter={handleDragEnter}
				on:dragleave={handleDragLeave}
				on:mousedown={handleTracksMouseDown}
				on:wheel={handleTracksWheel}
				on:scroll={onTracksScroll}
			>
				<div
					class="tracks-inner"
					style="width: {arrangeContentWidthPx}px; min-width: {arrangeContentWidthPx}px"
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
					class:clip-ghost--new-track={isNewArrangeTrackRow(fileDragPreview.trackIndex, tracks.length)}
					style="top: {fileDragPreview.trackIndex * ARRANGE_TRACK_HEIGHT}px; left: {fileDragPreview.startSec * scale}px; width: {fileDragPreview.durationSec != null ? fileDragPreview.durationSec * scale : 150}px;"
				></div>
			{/if}
			{#if tracksPointer.kind === 'selbox'}
				<div
					class="arrange-selbox"
					style="
						left: {Math.min(tracksPointer.x0, tracksPointer.x1)}px;
						top: {Math.min(tracksPointer.y0, tracksPointer.y1) - tracksScrollTop}px;
						width: {Math.abs(tracksPointer.x1 - tracksPointer.x0)}px;
						height: {Math.abs(tracksPointer.y1 - tracksPointer.y0)}px;
					"
				></div>
			{/if}
			{#each tracks as track, trackIndex}
				<Track
					clips={track.clips}
					midiClips={track.midi_clips ?? []}
					{scale}
					{trackIndex}
					bpm={$tempo.bpm}
					trackColor={TRACK_COLORS[trackIndex % TRACK_COLORS.length]}
					on:clipdragstart={onClipDragStart}
					on:clipresizestart={onClipResizeStart}
					on:clipcontextmenu={handleClipContextMenu}
					on:midiclip_dragstart={onMidiClipDragStart}
					on:midiclip_resizestart={onMidiClipResizeStart}
					on:midiclip_dblclick={onMidiClipDblClick}
					on:midiclip_activate={onMidiClipActivate}
					on:midiclip_contextmenu={onMidiClipContextMenu}
					on:trackdblclick={handleTrackDoubleClick}
				/>
			{/each}
				</div>
			</div>
			<ZoomScrollbar
				orientation="h"
				start={hScrollStart}
				end={hScrollEnd}
				on:change={onArrangeHScrollChange}
			/>
		</div>
	</div>

</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.container {
		display: flex;
		flex: 1;
		min-height: 0;
		position: relative;
		overflow: hidden;
	}

	// ── Left panel ────────────────────────────────────────────────────────────
	.left-panel {
		width: 200px;
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		background-color: $panel-background-color;
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

		&.has-scroll-separator {
			border-top: 1px solid $border-color;
		}
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

	.tracks-column {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}

	.tracks {
		flex: 1;
		min-height: 0;
		overflow: auto;
		position: relative;
		cursor: crosshair;
		scrollbar-width: none;

		&::-webkit-scrollbar {
			display: none;
		}

		&.is-dragging {
			cursor: grabbing;
			user-select: none;
		}

		&.is-resizing {
			cursor: ew-resize;
			user-select: none;
		}

		&.is-file-dragging {
			cursor: copy;
		}

		&.is-selbox-dragging {
			cursor: crosshair;
			user-select: none;
		}
	}

	.tracks-inner {
		position: relative;
		min-height: 100%;
	}

	.arrange-selbox {
		position: absolute;
		box-sizing: border-box;
		pointer-events: none;
		z-index: 6;
		background: rgba(21, 113, 255, 0.07);
		border: 1px dashed rgba(21, 113, 255, 0.65);
		border-radius: 2px;
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

		&--new-track {
			background-color: rgba(107, 203, 119, 0.12);
			border-color: rgba(107, 203, 119, 0.75);
		}
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

</style>
