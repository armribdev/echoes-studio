<!-- PianoRoll.svelte — éditeur MIDI intégré -->
<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { get } from 'svelte/store';
	import {
		pianoRollStore,
		pianoRollNotes,
		pianoRollSelection,
		type PianoRollMode
	} from '../stores/pianoRoll';
	import { drawGridLayer } from '../frontend/pianoRoll/gridRenderer';
	import { drawNotesLayer } from '../frontend/pianoRoll/notesRenderer';
	import { drawVelocityLayer, VEL_H } from '../frontend/pianoRoll/velocityRenderer';
	import {
		NOTE_H,
		TOTAL_PITCHES,
		snapBeat,
		hitTestNote,
		isResizeHandle,
		visibleBeatsForEdit,
		pitchFromGridY
	} from '../frontend/pianoRoll/coords';
	import { transport } from '../stores/transport';
	import { tempo } from '../stores/tempo';
	import type { MidiControlPoint, MidiNote } from '../types/global';
	import { engineProject } from '../frontend/store/engineStore';
	import { midiContentOf } from '../frontend/selectors';
	import {
		setClipMidiControls,
		setClipMidiNotes,
		undoCommand,
		redoCommand,
		seek
	} from '../frontend/commands/dispatcher';
	import { snapPosition } from '../lib/snapUtils';
	import { midiGridBeats } from '../stores/project';
	import { currentProjectMusical, updateProjectMusical } from '../frontend/commands/dispatcher';
	import { startTransportScrub } from '../frontend/transportScrub';
	import { shouldHandlePianoRollShortcuts } from '../frontend/shortcuts/context';
	import { matchesShortcut, matchesShortcutRow } from '../frontend/shortcuts/bindings';
	import { preferences } from '../stores/preferences';
	import { noteClipboard, copyMidiNotes } from '../stores/noteClipboard';
	import { watchPluginKeyNames } from '../stores/pluginKeyNames';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import { t } from 'svelte-i18n';
	import { pianoRollStatusInfo } from '../stores/status';
	import { closeEditor, enterEditFromPreview } from '../frontend/pianoRoll/clipEditor';
	import { bottomPanel } from '../stores/bottomPanel';
	import {
		getPreviewPitchRange,
		computePreviewPanelHeight,
		computePreviewRowH,
		PREVIEW_CHROME_H
	} from '../frontend/pianoRoll/previewLayout';
	import { previewPanelResizing, editPanelResizing } from '../frontend/pianoRoll/previewRefit';
	import { fitEditScrollForNotes, pitchRangeScrollTop } from '../frontend/pianoRoll/editScroll';
	import { auditionPitch, releaseAudition } from '../frontend/pianoRoll/noteAudition';
	import TimeRuler from './TimeRuler.svelte';
	import Playhead from './Playhead.svelte';
	import ZoomScrollbar from './ZoomScrollbar.svelte';
	import { RULER_HEIGHT } from '../frontend/timeRuler/constants';
	import PianoRollToolbar from './pianoRoll/PianoRollToolbar.svelte';
	import PianoRollKeys from './pianoRoll/PianoRollKeys.svelte';
	import PianoRollControlLanes from './pianoRoll/PianoRollControlLanes.svelte';
	import {
		LANE_HEIGHT,
		MAX_VISIBLE_LANES,
		candidateLanes,
		isExpressionLane,
		laneKey,
		lanePoints,
		lanesWithData,
		replaceLanePoints,
		type ControlLane
	} from '../frontend/pianoRoll/controlLanes';
	import {
		expressionLaneHasData,
		expressionLanesWithData,
		withExpressionPoints
	} from '../frontend/pianoRoll/noteExpressionLanes';
	const MIN_DURATION = 0.0625;

	const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
	const dpr = typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1;

	// ── État local ─────────────────────────────────────────────────────────────
	let canvasGrid: HTMLCanvasElement;
	let canvasVel: HTMLCanvasElement;
	let scrollContainer: HTMLDivElement;
	let velWrapper: HTMLDivElement;
	let keysContainer: HTMLDivElement | undefined;

	let scrollLeft = 0;
	let scrollTop = 0;

	// ── Scrollbars personnalisés ────────────────────────────────────────────────
	let hScrollStart = 0;
	let hScrollEnd = 1;
	let vScrollStart = 0;
	let vScrollEnd = 1;
	let hTotalBeats = 1; // beats totaux dans le canvas (pour zoom cohérent)

	// Hover (ghost note + position indicator)
	let hoverBeat = -1;
	let hoverPitch = -1;

	// Velocity drag
	let velDragActive = false;

	// Middle-click pan
	let isPanning = false;
	let panStartX = 0;
	let panStartScrollLeft = 0;

	type DragState =
		| { type: 'none' }
		| { type: 'create'; noteId: string; pitch: number; startBeat: number; anchorBeat: number }
		| {
				type: 'move';
				noteId: string;
				origPitch: number;
				origStart: number;
				anchorBeat: number;
				anchorPitch: number;
				isGroup: boolean;
				origPositions: Map<string, { pitch: number; start: number }>;
		  }
		| {
				type: 'resize';
				noteId: string;
				origDur: number;
				anchorBeat: number;
				isGroup: boolean;
				origDurs: Map<string, number>;
		  }
		| { type: 'erase' }
		| { type: 'selbox'; x0: number; y0: number; x1: number; y1: number };

	let drag: DragState = { type: 'none' };
	let isDragging = false;

	/** Notes en cours de drag (pas de mutation store jusqu'au mouseup). */
	let workingNotes: MidiNote[] | null = null;
	/** Sélection locale pendant selbox. */
	let dragSelIds: string[] | null = null;

	let playheadBeat = 0;
	let rulerViewportWidth = 0;
	let pianoRulerEl: HTMLDivElement | undefined;
	let scrubCleanup: (() => void) | null = null;

	let trackId: string | null = null;

	$: instrument = $engineProject.tracks.find((t) => t.id === trackId)?.instrument ?? null;
	$: watchPluginKeyNames(trackId && instrument ? `instrument:${trackId}` : null);

	$: clipStartSec = (() => {
		if (!trackId || !clipId) return 0;
		const track = $engineProject.tracks.find((t) => t.id === trackId);
		const clip = track?.midi_clips.find((c) => c.id === clipId);
		return clip?.start ?? 0;
	})();

	$: clipStartBeat = (clipStartSec / 60) * $tempo.bpm;
	$: playheadX = open ? (playheadBeat - clipStartBeat) * zoom : 0;
	$: playheadMarkerX = playheadX - scrollLeft;
	$: rulerMaxBars =
		Math.ceil(
			(editMode ? visibleBeatsForEdit(notes, clipDurationBeats) : Math.max(clipDurationBeats, 1)) /
				($tempo.numerator || 4)
		) + 4;

	function copySelectedNotes() {
		const sel = get(pianoRollStore).selectedNoteIds;
		copyMidiNotes(pianoRollStore.getNotes().filter((n) => sel.has(n.id)));
	}

	function cutSelectedNotes() {
		const sel = get(pianoRollStore).selectedNoteIds;
		if (sel.size === 0) return;
		copySelectedNotes();
		pianoRollStore.deleteSelectedNotes();
		commitNotes();
	}

	function selectedNotes(): MidiNote[] {
		const sel = get(pianoRollStore).selectedNoteIds;
		return pianoRollStore.getNotes().filter((n) => sel.has(n.id));
	}

	function nudgeSelected(deltaPitch: number, deltaStart: number) {
		const notes = selectedNotes();
		if (notes.length === 0) return;
		if (deltaStart < 0) {
			let minStart = notes[0].start;
			for (const n of notes) minStart = Math.min(minStart, n.start);
			deltaStart = Math.max(deltaStart, -minStart);
		}
		if (deltaPitch !== 0) {
			let minPitch = notes[0].pitch;
			let maxPitch = notes[0].pitch;
			for (const n of notes) {
				minPitch = Math.min(minPitch, n.pitch);
				maxPitch = Math.max(maxPitch, n.pitch);
			}
			deltaPitch = Math.max(-minPitch, Math.min(127 - maxPitch, deltaPitch));
		}
		if (deltaPitch === 0 && deltaStart === 0) return;
		pianoRollStore.moveSelectedNotes(deltaPitch, deltaStart);
		commitNotes();
	}

	function duplicateSelected() {
		const notes = selectedNotes();
		if (notes.length === 0) return;
		let minStart = notes[0].start;
		let maxEnd = notes[0].start + notes[0].duration;
		for (const n of notes) {
			minStart = Math.min(minStart, n.start);
			maxEnd = Math.max(maxEnd, n.start + n.duration);
		}
		const span = maxEnd - minStart;
		const copies: MidiNote[] = notes.map((n) => ({
			...n,
			id: crypto.randomUUID().slice(0, 8),
			start: n.start + span
		}));
		pianoRollStore.setNotes([...pianoRollStore.getNotes(), ...copies]);
		pianoRollStore.selectNotes(copies.map((n) => n.id));
		commitNotes();
	}

	function quantizeSelected() {
		const sel = get(pianoRollStore).selectedNoteIds;
		if (sel.size === 0 || quantize <= 0) return;
		const notes = pianoRollStore.getNotes().map((n) => {
			if (!sel.has(n.id)) return n;
			const start = Math.max(0, Math.round(n.start / quantize) * quantize);
			return start === n.start ? n : { ...n, start };
		});
		pianoRollStore.setNotes(notes);
		commitNotes();
	}

	function pasteNotes() {
		const copied = get(noteClipboard);
		if (copied.length === 0) return;
		const origin = Math.max(0, snapBeat(playheadBeat - clipStartBeat, quantize));
		const pasted: MidiNote[] = copied.map((note) => ({
			id: crypto.randomUUID().slice(0, 8),
			pitch: note.pitch,
			start: origin + note.relativeStart,
			duration: note.duration,
			velocity: note.velocity,
			...(note.expressions ? { expressions: note.expressions } : {})
		}));
		pianoRollStore.setNotes([...pianoRollStore.getNotes(), ...pasted]);
		pianoRollStore.selectNotes(pasted.map((n) => n.id));
		commitNotes();
	}

	function noteSignature(
		notes: { id: string; pitch: number; start: number; duration: number; velocity: number }[]
	): string {
		return notes.map((n) => `${n.id}:${n.pitch}:${n.start}:${n.duration}:${n.velocity}`).join('|');
	}

	let dragNoteSignature = '';

	// ── Lignes de contrôle (pitch bend, CC, aftertouch) ─────────────────────────

	const NO_CONTROLS: MidiControlPoint[] = [];
	let controlLanes: ControlLane[] = [];
	let controlLanesClipId: string | null = null;
	let laneHeight = LANE_HEIGHT;

	$: clipControls = (() => {
		if (!trackId || !clipId) return NO_CONTROLS;
		const track = $engineProject.tracks.find((t) => t.id === trackId);
		const clip = track?.midi_clips.find((c) => c.id === clipId);
		return clip ? midiContentOf($engineProject, clip).channel_controls : NO_CONTROLS;
	})();

	$: if (clipId !== controlLanesClipId) {
		controlLanesClipId = clipId;
		controlLanes = [...lanesWithData(clipControls), ...expressionLanesWithData(notes)].slice(
			0,
			MAX_VISIBLE_LANES
		);
	}

	$: expressionNote =
		$pianoRollSelection.size === 1
			? (notes.find((note) => $pianoRollSelection.has(note.id)) ?? null)
			: null;
	$: hasLaneData = (lane: ControlLane) =>
		isExpressionLane(lane)
			? expressionLaneHasData(expressionNote, lane)
			: lanePoints(clipControls, lane).length > 0;

	$: laneCandidates = candidateLanes(
		clipControls,
		controlLanes,
		notes.map((note) => note.pitch)
	);
	$: showAddRow = controlLanes.length < MAX_VISIBLE_LANES;
	$: lanesWidthPx = Math.max(hTotalBeats * zoom, 1);

	function commitControls(next: MidiControlPoint[]): Promise<unknown> {
		const { trackId, clipId } = get(pianoRollStore);
		if (!trackId || !clipId) return Promise.resolve();
		return setClipMidiControls(trackId, clipId, next);
	}

	function pickLane(key: string): ControlLane | undefined {
		return [...controlLanes, ...laneCandidates].find((lane) => laneKey(lane) === key);
	}

	function switchLane(index: number, key: string) {
		const lane = pickLane(key);
		if (lane) controlLanes = controlLanes.map((current, i) => (i === index ? lane : current));
	}

	function addLane(key: string) {
		const lane = pickLane(key);
		if (lane) controlLanes = [...controlLanes, lane];
	}

	function hideLane(index: number) {
		controlLanes = controlLanes.filter((_, i) => i !== index);
	}

	function commitNote(next: MidiNote): Promise<unknown> {
		const { trackId, clipId } = get(pianoRollStore);
		if (!trackId || !clipId) return Promise.resolve();
		const updated = pianoRollStore.getNotes().map((note) => (note.id === next.id ? next : note));
		pianoRollStore.setNotes(updated);
		return setClipMidiNotes(trackId, clipId, updated);
	}

	function clearLane(lane: ControlLane) {
		if (isExpressionLane(lane)) {
			if (expressionNote)
				commitNote(withExpressionPoints(expressionNote, lane, [])).catch(console.error);
			return;
		}
		commitControls(replaceLanePoints(clipControls, lane, [])).catch(console.error);
	}

	function applyQuantize(beats: number) {
		pianoRollStore.setQuantize(beats);
		midiGridBeats.set(beats);
		updateProjectMusical({ ...currentProjectMusical(), midi_grid_beats: beats }).catch(
			console.error
		);
	}

	async function commitNotes() {
		const { trackId, clipId } = get(pianoRollStore);
		if (!trackId || !clipId) return;
		try {
			await setClipMidiNotes(trackId, clipId, pianoRollStore.getNotes());
		} catch (e) {
			console.error('commitNotes failed:', e);
		}
	}

	let zoom: number;
	let quantize: number;
	let mode: PianoRollMode;
	let open: boolean;
	let trackColor: string;
	let clipDurationBeats: number;
	let clipId: string | null;
	let lastLayoutClipId: string | null = null;

	$: editMode = $keyboardFocus === 'piano-roll';

	let previewPitchMin = 57;
	let previewPitchMax = 72;
	let previewRowH = NOTE_H;

	// Doit être déclaré avant les souscriptions (temporal dead zone)
	let gridRafPending = false;
	let velRafPending = false;
	let layoutRunning = false;
	let gridResizePasses = 0;
	const MAX_GRID_RESIZE_PASSES = 4;

	/** Plage de hauteurs ancrée au début d'un redimensionnement en édition. */
	let editScrollAnchor: { minPitch: number; maxPitch: number } | null = null;
	let gridViewportObserver: ResizeObserver | undefined;

	const unsubStore = pianoRollStore.subscribe((s) => {
		const zoomChanged = s.zoom !== zoom;
		const quantChanged = s.quantize !== quantize;
		if (zoomChanged) zoom = s.zoom;
		if (quantChanged) quantize = s.quantize;
		if (s.mode !== mode) mode = s.mode;
		if (s.open !== open) open = s.open;
		if (s.trackColor !== trackColor) {
			trackColor = s.trackColor;
			if (open) scheduleNotesRedraw();
		}
		if (s.clipDurationBeats !== clipDurationBeats) clipDurationBeats = s.clipDurationBeats;
		if (s.clipId !== clipId) clipId = s.clipId;
		if (s.trackId !== trackId) trackId = s.trackId;
		const clipChanged = s.open && s.clipId && s.clipId !== lastLayoutClipId;
		if (clipChanged) lastLayoutClipId = s.clipId;
		else if (!s.open) lastLayoutClipId = null;
		if (open && zoomChanged) {
			tick().then(() => {
				resizeCanvases();
				scheduleGridRedraw();
				scheduleNotesRedraw();
			});
		} else if (open && quantChanged) {
			scheduleGridRedraw();
		} else if (clipChanged) {
			if (get(keyboardFocus) === 'piano-roll') {
				tick().then(() => initEditCanvases(true));
			} else {
				scheduleLayout(true);
			}
		}
	});
	onDestroy(unsubStore);

	let prevKeyboardFocus = get(keyboardFocus);
	const unsubFocus = keyboardFocus.subscribe((focus) => {
		if (focus === prevKeyboardFocus) return;
		if (prevKeyboardFocus === 'piano-roll') persistEditScrollFromView();
		prevKeyboardFocus = focus;
		if (!get(pianoRollStore).open) return;
		tick().then(async () => {
			if (!get(pianoRollStore).open) return;
			await tick();
			if (!get(pianoRollStore).open) return;
			if (focus === 'piano-roll') initEditCanvases(false);
			else scheduleLayout(false);
		});
	});
	onDestroy(unsubFocus);

	let notes: MidiNote[] = [];
	const unsubNotes = pianoRollNotes.subscribe((n) => {
		notes = n;
		if (!get(pianoRollStore).open) return;
		if (get(keyboardFocus) === 'piano-roll') {
			scheduleNotesRedraw();
			scheduleVelRedraw();
		} else {
			schedulePreviewLayout(false);
		}
	});
	onDestroy(unsubNotes);

	const unsubSelection = pianoRollSelection.subscribe(() => {
		if (!get(pianoRollStore).open) return;
		scheduleNotesRedraw();
		scheduleVelRedraw();
	});
	onDestroy(unsubSelection);

	// Sync notes from Rust state (after undo/redo or MIDI_CONTENT_NOTES_SET) when not dragging.
	const unsubEngine = engineProject.subscribe(($project) => {
		if (isDragging || !get(pianoRollStore).open) return;
		const { trackId, clipId } = get(pianoRollStore);
		if (!trackId || !clipId) return;
		const track = $project.tracks.find((t) => t.id === trackId);
		if (!track) return;
		const clip = track.midi_clips.find((c) => c.id === clipId);
		if (clip) pianoRollStore.syncNotes(midiContentOf($project, clip).notes);
	});
	onDestroy(unsubEngine);

	const unsubTransport = transport.subscribe((t) => {
		const bpm = get(tempo).bpm;
		playheadBeat = (t.position / 60) * bpm;
	});
	onDestroy(unsubTransport);

	// ── Cycle de vie ──────────────────────────────────────────────────────────

	function displayNotes(): MidiNote[] {
		return workingNotes ?? notes;
	}

	function displaySelection(): Set<string> {
		if (dragSelIds) return new Set(dragSelIds);
		return get(pianoRollStore).selectedNoteIds;
	}

	function scheduleGridRedraw() {
		if (gridRafPending || !open) return;
		gridRafPending = true;
		requestAnimationFrame(() => {
			gridRafPending = false;
			drawCanvas();
		});
	}

	function scheduleNotesRedraw() {
		scheduleGridRedraw();
	}

	function scheduleVelRedraw() {
		if (velRafPending || !editMode || !open) return;
		velRafPending = true;
		requestAnimationFrame(() => {
			velRafPending = false;
			drawVelocity();
		});
	}

	function scheduleHoverRedraw() {
		scheduleGridRedraw();
	}

	const unsubEditResizing = editPanelResizing.subscribe((resizing) => {
		if (resizing && get(pianoRollStore).open && get(keyboardFocus) === 'piano-roll') {
			const { min, max } = getPreviewPitchRange(displayNotes());
			editScrollAnchor = { minPitch: min, maxPitch: max };
			adjustEditScrollForPanelResize();
			return;
		}
		if (!resizing) {
			editScrollAnchor = null;
			if (get(pianoRollStore).open && get(keyboardFocus) === 'piano-roll') {
				persistEditScrollFromView();
			}
		}
	});
	onDestroy(unsubEditResizing);

	/** Recalcule rowH + canvas en aperçu après redimensionnement (viewport mesuré). */
	function adjustPreviewLayoutForPanelResize() {
		if (!scrollContainer || !open || editMode) return;

		syncPreviewLayoutState();
		resizeCanvases();
		drawCanvas();
	}

	/** Recentre la plage de notes sans redessiner (le canvas ne change pas en hauteur). */
	function adjustEditScrollForPanelResize() {
		if (!scrollContainer || !editScrollAnchor || !editMode) return;

		const viewportH = scrollContainer.clientHeight;
		if (viewportH <= 0) return;

		const contentH = TOTAL_PITCHES * NOTE_H;
		const maxScrollTop = Math.max(0, contentH - viewportH);
		const top = Math.min(
			pitchRangeScrollTop(editScrollAnchor.minPitch, editScrollAnchor.maxPitch, viewportH),
			maxScrollTop
		);

		scrollContainer.scrollTop = top;
		scrollTop = top;
		if (keysContainer) keysContainer.scrollTop = top;
	}

	async function scheduleLayout(autoHeight = false) {
		if (!open || layoutRunning) return;
		layoutRunning = true;
		try {
			await tick();
			await tick();
			if (!open) return;
			if (get(keyboardFocus) === 'piano-roll') initEditCanvases(false);
			else await fitPreviewView(autoHeight);
		} finally {
			layoutRunning = false;
		}
	}

	function schedulePreviewLayout(autoHeight = false) {
		if (!open || get(keyboardFocus) === 'piano-roll') return;
		scheduleLayout(autoHeight);
	}

	function initEditCanvases(fitScroll = false) {
		resizeCanvases();
		applyEditScrollView(fitScroll);
		scheduleGridRedraw();
		scheduleNotesRedraw();
		scheduleVelRedraw();
	}

	function persistEditScrollFromView() {
		if (!scrollContainer) return;
		const { trackId, clipId } = get(pianoRollStore);
		if (!trackId || !clipId) return;
		pianoRollStore.saveEditScroll(
			trackId,
			clipId,
			scrollContainer.scrollLeft,
			scrollContainer.scrollTop
		);
	}

	function setScrollContainerPosition(left: number, top: number) {
		if (!scrollContainer) return;
		scrollContainer.scrollLeft = left;
		scrollContainer.scrollTop = top;
		scrollLeft = left;
		scrollTop = top;
		if (keysContainer) keysContainer.scrollTop = top;
		if (velWrapper) velWrapper.scrollLeft = left;
	}

	/** Restaure le scroll mémorisé ou cadre la plage de notes (première visite / clip changé). */
	function applyEditScrollView(forceFit = false) {
		if (!scrollContainer || !editMode) return;

		const { trackId, clipId } = get(pianoRollStore);
		const viewportH = scrollContainer.clientHeight;
		const contentH = TOTAL_PITCHES * NOTE_H;
		const maxScrollTop = Math.max(0, contentH - viewportH);
		const contentW = Math.max(scrollContainer.clientWidth, visibleBeats() * zoom + 400);
		const maxScrollLeft = Math.max(0, contentW - scrollContainer.clientWidth);

		let left = 0;
		let top = 0;

		if (!forceFit) {
			const saved = pianoRollStore.getEditScroll(trackId, clipId);
			if (saved) {
				left = Math.min(saved.scrollLeft, maxScrollLeft);
				top = Math.min(saved.scrollTop, maxScrollTop);
				setScrollContainerPosition(left, top);
				return;
			}
		}

		const fit = fitEditScrollForNotes(displayNotes(), viewportH);
		left = Math.min(fit.scrollLeft, maxScrollLeft);
		top = Math.min(fit.scrollTop, maxScrollTop);
		setScrollContainerPosition(left, top);
	}

	// HiDPI helper
	function setSize(c: HTMLCanvasElement, w: number, h: number) {
		c.width = Math.round(w * dpr);
		c.height = Math.round(h * dpr);
		c.style.width = w + 'px';
		c.style.height = h + 'px';
	}

	function getCtx(canvas: HTMLCanvasElement): CanvasRenderingContext2D | null {
		const ctx = canvas.getContext('2d');
		if (!ctx) return null;
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		return ctx;
	}

	function resizeCanvases() {
		if (!canvasGrid || !scrollContainer) return;
		rulerViewportWidth = scrollContainer.clientWidth;
		let totalH: number;
		let w: number;
		if (editMode) {
			const totalBeats = visibleBeats();
			totalH = TOTAL_PITCHES * NOTE_H;
			w = Math.max(scrollContainer.clientWidth, totalBeats * zoom + 400);
		} else {
			const pitchCount = previewPitchMax - previewPitchMin + 1;
			totalH = pitchCount * previewRowH;
			const duration = Math.max(get(pianoRollStore).clipDurationBeats, 0.25);
			w = Math.max(scrollContainer.clientWidth, duration * zoom + 8);
		}
		setSize(canvasGrid, w, totalH);
		if (canvasVel) setSize(canvasVel, w, VEL_H);
		if (editMode) updateScrollbars();
	}

	function noteRowH(): number {
		return editMode ? NOTE_H : previewRowH;
	}

	/** Met à jour la plage de pitch aperçu depuis les notes courantes (évite notes hors-canvas). */
	function syncPreviewLayoutState(panelH = get(bottomPanel).height) {
		const range = getPreviewPitchRange(notes);
		previewPitchMin = range.min;
		previewPitchMax = range.max;
		const measuredGridH = scrollContainer?.clientHeight ?? 0;
		const effectivePanelH = measuredGridH > 0 ? measuredGridH + PREVIEW_CHROME_H : panelH;
		previewRowH = computePreviewRowH(notes, effectivePanelH);
	}

	async function applyPreviewFit(
		panelH: number,
		opts: { fitZoom?: boolean; resetScroll?: boolean } = {}
	) {
		if (!scrollContainer || get(keyboardFocus) === 'piano-roll') return;

		const { fitZoom = true, resetScroll = false } = opts;

		syncPreviewLayoutState(panelH);
		gridResizePasses = MAX_GRID_RESIZE_PASSES;

		if (fitZoom) {
			const duration = Math.max(get(pianoRollStore).clipDurationBeats, 0.25);
			const viewportW = Math.max(80, scrollContainer.clientWidth - 4);
			const fitZoomVal = Math.max(12, Math.min(400, (viewportW * 0.96) / duration));
			if (Math.abs(fitZoomVal - zoom) > 0.5) {
				pianoRollStore.setZoom(fitZoomVal);
			}
		}

		if (resetScroll) {
			scrollContainer.scrollLeft = 0;
			scrollContainer.scrollTop = 0;
		}

		resizeCanvases();
		scheduleGridRedraw();
	}

	/** @param autoHeight — calcule et applique la hauteur idéale (ouverture clip / changement de clip). */
	async function fitPreviewView(autoHeight = false) {
		if (!scrollContainer || get(keyboardFocus) === 'piano-roll') return;
		if (get(previewPanelResizing)) return;

		let panelH = get(bottomPanel).height;
		if (autoHeight) {
			panelH = computePreviewPanelHeight(notes);
			bottomPanel.setPreviewAutoHeight(panelH);
			await tick();
			panelH = get(bottomPanel).height;
		}

		await applyPreviewFit(panelH, { fitZoom: true, resetScroll: autoHeight });
	}

	function visibleBeats(): number {
		if (!editMode) return Math.max(get(pianoRollStore).clipDurationBeats, 1);
		return visibleBeatsForEdit(displayNotes(), clipDurationBeats);
	}

	// Indicateur de position (toolbar)
	function beatLabel(beat: number): string {
		const num = get(tempo).numerator || 4;
		const measure = Math.floor(beat / num) + 1;
		const b = Math.floor(beat % num) + 1;
		return `${measure}:${b}`;
	}

	$: posLabel =
		hoverPitch >= 0 && hoverBeat >= 0
			? `${NOTE_NAMES[hoverPitch % 12]}${Math.floor(hoverPitch / 12) - 1}  ${beatLabel(hoverBeat)}`
			: '';

	$: clipMetaLabel = (() => {
		const num = $tempo.numerator || 4;
		const bars = clipDurationBeats / num;
		const barsRounded = Math.round(bars * 10) / 10;
		const barsText = Number.isInteger(barsRounded) ? `${barsRounded}` : `${barsRounded.toFixed(1)}`;
		return `${barsText} mes.`;
	})();

	$: noteMetaLabel = `${notes.length} note${notes.length !== 1 ? 's' : ''}`;

	$: pianoRollStatusInfo.set(
		open
			? {
					hoverLabel: posLabel,
					noteMeta: noteMetaLabel,
					clipMeta: clipMetaLabel
				}
			: null
	);

	onDestroy(() => {
		pianoRollStatusInfo.set(null);
	});

	// ── Dessin (grille + notes en couches) ────────────────────────────────────

	function drawCanvas() {
		if (drawGrid()) drawNotes();
	}

	function drawGrid(): boolean {
		if (!canvasGrid) return false;
		const ctx = getCtx(canvasGrid);
		if (!ctx) return false;

		const W = canvasGrid.width / dpr;
		const H = canvasGrid.height / dpr;

		if (!editMode && scrollContainer && !get(previewPanelResizing)) {
			const pitchCount = previewPitchMax - previewPitchMin + 1;
			const expectedH = pitchCount * previewRowH;
			const duration = Math.max(get(pianoRollStore).clipDurationBeats, 0.25);
			const expectedW = Math.max(scrollContainer.clientWidth, duration * zoom + 8);
			if (
				gridResizePasses < MAX_GRID_RESIZE_PASSES &&
				(Math.abs(H - expectedH) > 1 || Math.abs(W - expectedW) > 1)
			) {
				gridResizePasses++;
				resizeCanvases();
				scheduleGridRedraw();
				return false;
			}
		}

		drawGridLayer({
			ctx,
			width: W,
			height: H,
			editMode,
			zoom,
			quantize,
			scrollLeft,
			viewportWidth: scrollContainer?.clientWidth ?? W,
			previewPitchMin,
			previewPitchMax,
			previewRowH
		});
		return true;
	}

	function drawNotes() {
		if (!canvasGrid) return;
		const ctx = getCtx(canvasGrid);
		if (!ctx) return;
		const W = canvasGrid.width / dpr;
		const H = canvasGrid.height / dpr;

		drawNotesLayer({
			ctx,
			width: W,
			height: H,
			clearBackground: false,
			editMode,
			zoom,
			quantize,
			mode,
			trackColor,
			notes: displayNotes(),
			selectedNoteIds: displaySelection(),
			previewPitchMin,
			previewPitchMax,
			previewRowH,
			hoverBeat,
			hoverPitch,
			dragType: drag.type,
			showSelbox: drag.type === 'selbox',
			selbox: drag.type === 'selbox' ? drag : undefined
		});
	}

	function drawVelocity() {
		if (!canvasVel) return;
		const ctx = getCtx(canvasVel);
		if (!ctx) return;
		drawVelocityLayer({
			ctx,
			width: canvasVel.width / dpr,
			notes: displayNotes(),
			selectedNoteIds: displaySelection(),
			zoom,
			quantize,
			trackColor
		});
	}
	// ── Interactions Canvas ───────────────────────────────────────────────────

	function getGridCoords(e: MouseEvent): {
		beat: number;
		pitch: number;
		rawX: number;
		rawY: number;
	} {
		if (!canvasGrid) return { beat: 0, pitch: 0, rawX: 0, rawY: 0 };
		const rect = canvasGrid.getBoundingClientRect();
		const rawX = e.clientX - rect.left;
		const rawY = e.clientY - rect.top;
		const rowH = noteRowH();
		return {
			beat: rawX / zoom,
			pitch: editMode
				? pitchFromGridY(rawY, rowH)
				: Math.max(
						previewPitchMin,
						Math.min(previewPitchMax, previewPitchMax - Math.floor(rawY / rowH))
					),
			rawX,
			rawY
		};
	}

	function hitTest(beat: number, pitch: number): MidiNote | undefined {
		return hitTestNote(displayNotes(), beat, pitch);
	}

	function noteResizeHandle(note: MidiNote, beatX: number): boolean {
		return isResizeHandle(note, beatX, zoom);
	}

	function beginWorkingNotes() {
		workingNotes = displayNotes().map((n) => ({ ...n }));
	}

	function resizeWorkingNote(id: string, duration: number) {
		if (!workingNotes) return;
		const d = Math.max(MIN_DURATION, duration);
		workingNotes = workingNotes.map((n) => (n.id === id ? { ...n, duration: d } : n));
		scheduleNotesRedraw();
		scheduleVelRedraw();
	}

	function resizeWorkingSelected(origDurs: Map<string, number>, delta: number) {
		if (!workingNotes) return;
		workingNotes = workingNotes.map((n) => {
			const orig = origDurs.get(n.id);
			if (orig === undefined) return n;
			return { ...n, duration: Math.max(MIN_DURATION, orig + delta) };
		});
		scheduleNotesRedraw();
		scheduleVelRedraw();
	}

	function beginResize(hit: MidiNote, beat: number, allowGroup: boolean) {
		beginWorkingNotes();
		const sel = get(pianoRollStore).selectedNoteIds;
		const isGroup = allowGroup && sel.has(hit.id) && sel.size > 1;
		const origDurs = new Map<string, number>();
		if (isGroup && workingNotes) {
			for (const n of workingNotes) {
				if (sel.has(n.id)) origDurs.set(n.id, n.duration);
			}
		}
		drag = {
			type: 'resize',
			noteId: hit.id,
			origDur: hit.duration,
			anchorBeat: beat,
			isGroup,
			origDurs
		};
	}

	function moveWorkingNote(id: string, pitch: number, start: number) {
		if (!workingNotes) return;
		workingNotes = workingNotes.map((n) =>
			n.id === id
				? { ...n, pitch: Math.max(0, Math.min(127, pitch)), start: Math.max(0, start) }
				: n
		);
		scheduleNotesRedraw();
	}

	function beginMove(hit: MidiNote, beat: number, pitch: number, isGroup: boolean) {
		beginWorkingNotes();
		const origPositions = new Map<string, { pitch: number; start: number }>();
		if (isGroup && workingNotes) {
			const sel = get(pianoRollStore).selectedNoteIds;
			for (const n of workingNotes) {
				if (sel.has(n.id)) origPositions.set(n.id, { pitch: n.pitch, start: n.start });
			}
		}
		drag = {
			type: 'move',
			noteId: hit.id,
			origPitch: hit.pitch,
			origStart: hit.start,
			anchorBeat: beat,
			anchorPitch: pitch,
			isGroup,
			origPositions
		};
		if (trackId) auditionPitch(trackId, hit.pitch, hit.velocity);
	}

	function moveWorkingGroup(
		origPositions: Map<string, { pitch: number; start: number }>,
		deltaPitch: number,
		deltaStart: number
	) {
		if (!workingNotes) return;
		workingNotes = workingNotes.map((n) => {
			const orig = origPositions.get(n.id);
			if (!orig) return n;
			return { ...n, pitch: orig.pitch + deltaPitch, start: orig.start + deltaStart };
		});
		scheduleNotesRedraw();
	}

	function eraseWorkingNote(id: string) {
		if (!workingNotes) return;
		workingNotes = workingNotes.filter((n) => n.id !== id);
		scheduleNotesRedraw();
		scheduleVelRedraw();
	}

	function flushWorkingNotes() {
		if (workingNotes) {
			pianoRollStore.setNotes(workingNotes);
			workingNotes = null;
		}
		if (dragSelIds) {
			pianoRollStore.selectNotes(dragSelIds);
			dragSelIds = null;
		}
	}

	function onGridMousedown(e: MouseEvent) {
		// Middle-click : pan horizontal (preview + édition)
		if (e.button === 1) {
			e.preventDefault();
			isPanning = true;
			panStartX = e.clientX;
			panStartScrollLeft = scrollContainer.scrollLeft;
			return;
		}
		if (!editMode) {
			if (e.button === 0) {
				e.preventDefault();
				enterEditFromPreview();
			}
			return;
		}
		if (e.button !== 0 && e.button !== 2) return;
		e.preventDefault();
		isDragging = true;
		dragNoteSignature = noteSignature(pianoRollStore.getNotes());
		const { beat, pitch, rawX, rawY } = getGridCoords(e);
		const hit = hitTest(beat, pitch);

		if (mode === 'draw') {
			beginWorkingNotes();
			if (hit) {
				if (noteResizeHandle(hit, beat)) {
					beginResize(hit, beat, true);
				} else {
					beginMove(hit, beat, pitch, false);
				}
			} else {
				const sb = snapBeat(beat, quantize);
				const id = crypto.randomUUID().slice(0, 8);
				workingNotes = [
					...displayNotes(),
					{ id, pitch, start: sb, duration: quantize, velocity: 100 }
				];
				drag = { type: 'create', noteId: id, pitch, startBeat: sb, anchorBeat: beat };
				if (trackId) auditionPitch(trackId, pitch, 100);
				scheduleNotesRedraw();
				scheduleVelRedraw();
			}
		} else if (mode === 'select') {
			if (hit) {
				if (noteResizeHandle(hit, beat)) {
					beginResize(hit, beat, !(e.ctrlKey || e.metaKey));
				} else {
					const isSelected = get(pianoRollStore).selectedNoteIds.has(hit.id);
					if (!isSelected) pianoRollStore.selectNote(hit.id, e.ctrlKey || e.metaKey);
					const grp = !(e.ctrlKey || e.metaKey) && get(pianoRollStore).selectedNoteIds.size > 1;
					beginMove(hit, beat, pitch, grp);
				}
			} else {
				if (!(e.ctrlKey || e.metaKey)) pianoRollStore.clearSelection();
				workingNotes = null;
				dragSelIds = [];
				drag = { type: 'selbox', x0: rawX, y0: rawY, x1: rawX, y1: rawY };
			}
		} else if (mode === 'erase') {
			beginWorkingNotes();
			if (hit) eraseWorkingNote(hit.id);
			drag = { type: 'erase' };
		}
	}

	function onGridMousemove(e: MouseEvent) {
		const { beat, pitch, rawX, rawY } = getGridCoords(e);

		if (editMode) {
			hoverBeat = beat;
			hoverPitch = Math.max(0, Math.min(127, pitch));
		}

		if (!editMode || drag.type === 'none') {
			if (editMode) updateCursor(e);
			scheduleHoverRedraw();
			return;
		}

		if (drag.type === 'create') {
			const newDur = Math.max(MIN_DURATION, snapBeat(beat, quantize) - drag.startBeat + quantize);
			resizeWorkingNote(drag.noteId, newDur);
		} else if (drag.type === 'move') {
			const moveDrag = drag;
			if (moveDrag.isGroup) {
				const newStart = Math.max(
					0,
					snapBeat(beat - (moveDrag.anchorBeat - moveDrag.origStart), quantize)
				);
				let deltaStart = newStart - moveDrag.origStart;
				let deltaPitch = pitch - moveDrag.anchorPitch;
				let minStart = Infinity;
				let minPitch = 127;
				let maxPitch = 0;
				for (const orig of moveDrag.origPositions.values()) {
					minStart = Math.min(minStart, orig.start);
					minPitch = Math.min(minPitch, orig.pitch);
					maxPitch = Math.max(maxPitch, orig.pitch);
				}
				if (minStart + deltaStart < 0) deltaStart = -minStart;
				deltaPitch = Math.max(-minPitch, Math.min(127 - maxPitch, deltaPitch));
				moveWorkingGroup(moveDrag.origPositions, deltaPitch, deltaStart);
				if (trackId) {
					const heard = Math.max(0, Math.min(127, moveDrag.origPitch + deltaPitch));
					const vel = workingNotes?.find((n) => n.id === moveDrag.noteId)?.velocity ?? 100;
					auditionPitch(trackId, heard, vel);
				}
			} else {
				const newStart = Math.max(
					0,
					snapBeat(beat - (moveDrag.anchorBeat - moveDrag.origStart), quantize)
				);
				const newPitch = Math.max(
					0,
					Math.min(127, moveDrag.origPitch + (pitch - moveDrag.anchorPitch))
				);
				moveWorkingNote(moveDrag.noteId, newPitch, newStart);
				if (trackId) {
					const vel = workingNotes?.find((n) => n.id === moveDrag.noteId)?.velocity ?? 100;
					auditionPitch(trackId, newPitch, vel);
				}
			}
		} else if (drag.type === 'resize') {
			const delta = snapBeat(beat - drag.anchorBeat, quantize);
			if (drag.isGroup) {
				resizeWorkingSelected(drag.origDurs, delta);
			} else {
				resizeWorkingNote(drag.noteId, drag.origDur + delta);
			}
		} else if (drag.type === 'erase') {
			const hit = hitTest(beat, pitch);
			if (hit) eraseWorkingNote(hit.id);
		} else if (drag.type === 'selbox') {
			drag = { ...drag, x1: rawX, y1: rawY };
			const bLeft = Math.min(drag.x0, drag.x1) / zoom;
			const bRight = Math.max(drag.x0, drag.x1) / zoom;
			const pTop = TOTAL_PITCHES - 1 - Math.floor(Math.min(drag.y0, drag.y1) / NOTE_H);
			const pBot = TOTAL_PITCHES - 1 - Math.floor(Math.max(drag.y0, drag.y1) / NOTE_H);
			dragSelIds = displayNotes()
				.filter(
					(n) =>
						n.pitch >= pBot && n.pitch <= pTop && n.start < bRight && n.start + n.duration > bLeft
				)
				.map((n) => n.id);
			scheduleNotesRedraw();
		}
	}

	function onGridMouseup(e: MouseEvent) {
		if (drag.type === 'move' && !drag.isGroup) {
			pianoRollStore.selectNote(drag.noteId, false);
		}
		const edited =
			drag.type === 'create' ||
			drag.type === 'move' ||
			drag.type === 'resize' ||
			drag.type === 'erase';
		flushWorkingNotes();
		const notesChanged = noteSignature(pianoRollStore.getNotes()) !== dragNoteSignature;
		drag = { type: 'none' };
		isDragging = false;
		if (edited && notesChanged) commitNotes();
		if (edited) releaseAudition();
		updateCursor(e);
	}

	function onGridMouseleave() {
		hoverBeat = -1;
		hoverPitch = -1;
		scheduleHoverRedraw();
	}

	function updateCursor(e: MouseEvent) {
		if (!canvasGrid) return;
		if (isPanning) {
			canvasGrid.style.cursor = 'grabbing';
			return;
		}
		const { beat, pitch } = getGridCoords(e);
		const hit = hitTest(beat, pitch);
		if (mode === 'draw') {
			canvasGrid.style.cursor = hit
				? noteResizeHandle(hit, beat)
					? 'ew-resize'
					: 'grab'
				: 'crosshair';
		} else if (mode === 'select') {
			canvasGrid.style.cursor = hit
				? noteResizeHandle(hit, beat)
					? 'ew-resize'
					: 'grab'
				: 'default';
		} else {
			canvasGrid.style.cursor = 'cell';
		}
	}

	function onGridContextmenu(e: MouseEvent) {
		if (!editMode) return;
		e.preventDefault();
		const { beat, pitch } = getGridCoords(e);
		const hit = hitTest(beat, pitch);
		if (hit) {
			pianoRollStore.deleteNote(hit.id);
			commitNotes();
		}
	}

	// ── Velocity interactions ──────────────────────────────────────────────────

	function updateVelocity(e: MouseEvent) {
		if (!canvasVel) return;
		const rect = canvasVel.getBoundingClientRect();
		const rawX = e.clientX - rect.left + (velWrapper?.scrollLeft ?? 0);
		const beat = rawX / zoom;
		const hit = displayNotes().find((n) => beat >= n.start && beat < n.start + n.duration);
		if (!hit) return;
		const vel = Math.max(
			1,
			Math.min(127, Math.round(((VEL_H - (e.clientY - rect.top)) / VEL_H) * 127))
		);
		if (!workingNotes) beginWorkingNotes();
		if (workingNotes) {
			workingNotes = workingNotes.map((n) => (n.id === hit.id ? { ...n, velocity: vel } : n));
			scheduleNotesRedraw();
			scheduleVelRedraw();
		}
	}

	function onVelMousedown(e: MouseEvent) {
		if (!editMode) return;
		if (e.button !== 0) return;
		velDragActive = true;
		dragNoteSignature = noteSignature(pianoRollStore.getNotes());
		updateVelocity(e);
	}

	function onVelMousemove(e: MouseEvent) {
		if (!velDragActive) return;
		updateVelocity(e);
	}

	function onVelMouseup() {
		velDragActive = false;
		flushWorkingNotes();
		if (noteSignature(pianoRollStore.getNotes()) !== dragNoteSignature) commitNotes();
	}

	// ── Scroll synchronisé ────────────────────────────────────────────────────

	function onGridScroll(e: Event) {
		const t = e.currentTarget as HTMLDivElement;
		const leftChanged = Math.abs(t.scrollLeft - scrollLeft) > 0.5;
		scrollLeft = t.scrollLeft;
		scrollTop = t.scrollTop;
		if (keysContainer) keysContainer.scrollTop = scrollTop;
		if (velWrapper) velWrapper.scrollLeft = scrollLeft;
		// En édition, le canvas couvre toute la hauteur : seul le pan horizontal nécessite un redraw.
		if (editMode && !leftChanged) return;
		if (!editMode && get(previewPanelResizing)) return;
		if (!editMode && !leftChanged) return;
		scheduleGridRedraw();
		if (editMode) updateScrollbars();
	}

	// ── Scrollbars personnalisés ──────────────────────────────────────────────

	function updateScrollbars() {
		if (!scrollContainer) return;
		const totalW = scrollContainer.scrollWidth;
		const vpW = scrollContainer.clientWidth;
		const totalH = scrollContainer.scrollHeight;
		const vpH = scrollContainer.clientHeight;

		hTotalBeats = zoom > 0 ? totalW / zoom : 1;
		hScrollStart = totalW > vpW ? scrollLeft / totalW : 0;
		hScrollEnd = totalW > vpW ? Math.min(1, (scrollLeft + vpW) / totalW) : 1;
		vScrollStart = totalH > vpH ? scrollTop / totalH : 0;
		vScrollEnd = totalH > vpH ? Math.min(1, (scrollTop + vpH) / totalH) : 1;
	}

	function onHScrollChange(e: CustomEvent<{ start: number; end: number; cause: string }>) {
		if (!scrollContainer || !editMode) return;
		const { start: ns, end: ne } = e.detail;

		const totalBeats = hTotalBeats;
		const vpW = scrollContainer.clientWidth;
		const leftBeat = ns * totalBeats;
		const rightBeat = ne * totalBeats;
		const spanBeats = rightBeat - leftBeat;
		if (spanBeats <= 0) return;

		const newZoom = Math.max(12, Math.min(400, vpW / spanBeats));
		const newScrollLeft = Math.max(0, leftBeat * newZoom);

		// Mise à jour visuelle immédiate pour fluidité
		hScrollStart = ns;
		hScrollEnd = ne;

		if (Math.abs(newZoom - zoom) < 0.5) {
			// Pan pur
			scrollContainer.scrollLeft = newScrollLeft;
			scrollLeft = newScrollLeft;
			if (velWrapper) velWrapper.scrollLeft = newScrollLeft;
			scheduleGridRedraw();
		} else {
			// Changement de zoom
			pianoRollStore.setZoom(newZoom);
			tick().then(() => {
				if (scrollContainer) {
					scrollContainer.scrollLeft = newScrollLeft;
					scrollLeft = newScrollLeft;
					if (velWrapper) velWrapper.scrollLeft = newScrollLeft;
				}
				resizeCanvases();
				scheduleGridRedraw();
				updateScrollbars();
			});
		}
	}

	function onVScrollChange(e: CustomEvent<{ start: number; end: number; cause: string }>) {
		if (!scrollContainer || !editMode) return;
		const { start: ns, end: ne, cause } = e.detail;

		const totalH = scrollContainer.scrollHeight;
		const vpH = scrollContainer.clientHeight;
		const thumbSz = vScrollEnd - vScrollStart;

		let newScrollTop: number;

		if (cause === 'resize-start') {
			// Poignée haute : fait défiler vers la position de début
			newScrollTop = ns * totalH;
			vScrollStart = ns;
			vScrollEnd = Math.min(1, ns + thumbSz);
		} else if (cause === 'resize-end') {
			// Poignée basse : fait défiler pour que la fin soit visible
			newScrollTop = (ne - thumbSz) * totalH;
			vScrollStart = Math.max(0, ne - thumbSz);
			vScrollEnd = ne;
		} else {
			// Pan
			newScrollTop = ns * totalH;
			vScrollStart = ns;
			vScrollEnd = ne;
		}

		newScrollTop = Math.max(0, Math.min(totalH - vpH, newScrollTop));
		scrollContainer.scrollTop = newScrollTop;
		scrollTop = newScrollTop;
		if (keysContainer) keysContainer.scrollTop = newScrollTop;
		scheduleGridRedraw();
	}

	// ── Keyboard ─────────────────────────────────────────────────────────────

	function onKeydown(e: KeyboardEvent) {
		if (!shouldHandlePianoRollShortcuts(e)) return;

		const overrides = get(preferences).shortcuts;
		let handled = false;

		if (matchesShortcut(e, 'pianoRoll.modeDraw', overrides)) {
			pianoRollStore.setMode('draw');
			handled = true;
		} else if (matchesShortcutRow(e, 'pianoRoll.modeSelect', overrides)) {
			pianoRollStore.setMode('select');
			handled = true;
		} else if (matchesShortcut(e, 'pianoRoll.modeErase', overrides)) {
			pianoRollStore.setMode('erase');
			handled = true;
		} else if (matchesShortcutRow(e, 'pianoRoll.quantize', overrides)) {
			e.preventDefault();
			if (!e.repeat) quantizeSelected();
			handled = true;
		}

		// Nudge: arrows stay fixed (Shift = octave) — awkward to rebind cleanly.
		if (!handled && e.key.startsWith('Arrow') && !e.ctrlKey && !e.metaKey && !e.altKey) {
			e.preventDefault();
			if (e.key === 'ArrowLeft') nudgeSelected(0, -quantize);
			else if (e.key === 'ArrowRight') nudgeSelected(0, quantize);
			else if (e.key === 'ArrowUp') nudgeSelected(e.shiftKey ? 12 : 1, 0);
			else if (e.key === 'ArrowDown') nudgeSelected(e.shiftKey ? -12 : -1, 0);
			handled = true;
		}

		if (!handled && matchesShortcutRow(e, 'pianoRoll.delete', overrides)) {
			e.preventDefault();
			pianoRollStore.deleteSelectedNotes();
			commitNotes();
			handled = true;
		} else if (!handled && matchesShortcut(e, 'pianoRoll.copy', overrides)) {
			e.preventDefault();
			copySelectedNotes();
			handled = true;
		} else if (!handled && matchesShortcut(e, 'pianoRoll.cut', overrides)) {
			e.preventDefault();
			cutSelectedNotes();
			handled = true;
		} else if (!handled && matchesShortcut(e, 'pianoRoll.paste', overrides)) {
			e.preventDefault();
			pasteNotes();
			handled = true;
		} else if (!handled && matchesShortcut(e, 'pianoRoll.duplicate', overrides)) {
			e.preventDefault();
			if (!e.repeat) duplicateSelected();
			handled = true;
		} else if (!handled && matchesShortcut(e, 'pianoRoll.selectAll', overrides)) {
			e.preventDefault();
			pianoRollStore.selectAll();
			handled = true;
		} else if (!handled && matchesShortcut(e, 'edit.undo', overrides)) {
			e.preventDefault();
			undoCommand();
			handled = true;
		} else if (
			!handled &&
			(matchesShortcut(e, 'edit.redo', overrides) || matchesShortcut(e, 'edit.redoAlt', overrides))
		) {
			e.preventDefault();
			redoCommand();
			handled = true;
		}

		if (handled) e.stopPropagation();
	}

	function onWindowMousemove(e: MouseEvent) {
		if (isPanning && scrollContainer) {
			const dx = e.clientX - panStartX;
			scrollContainer.scrollLeft = Math.max(0, panStartScrollLeft - dx);
		}
	}

	function onWindowMouseup() {
		if (isPanning) isPanning = false;
		const auditioning = drag.type === 'create' || drag.type === 'move';
		const needsCommit =
			auditioning || drag.type === 'resize' || drag.type === 'erase' || drag.type === 'selbox';
		if (drag.type !== 'none') {
			flushWorkingNotes();
			drag = { type: 'none' };
			isDragging = false;
		}
		if (velDragActive) {
			velDragActive = false;
			flushWorkingNotes();
			commitNotes();
		}
		if (needsCommit) commitNotes();
		if (auditioning) releaseAudition();
	}

	onMount(() => {
		window.addEventListener('keydown', onKeydown);
		window.addEventListener('mousemove', onWindowMousemove);
		window.addEventListener('mouseup', onWindowMouseup);
	});
	onDestroy(() => {
		releaseAudition();
		gridViewportObserver?.disconnect();
		scrubCleanup?.();
		window.removeEventListener('keydown', onKeydown);
		window.removeEventListener('mousemove', onWindowMousemove);
		window.removeEventListener('mouseup', onWindowMouseup);
	});

	$: if (scrollContainer && !gridViewportObserver) {
		gridViewportObserver = new ResizeObserver(() => {
			if (get(editPanelResizing) && editScrollAnchor) {
				adjustEditScrollForPanelResize();
				return;
			}
			if (get(previewPanelResizing) && get(keyboardFocus) !== 'piano-roll') {
				adjustPreviewLayoutForPanelResize();
			}
		});
		gridViewportObserver.observe(scrollContainer);
	}

	function timelineScale(): number {
		return (zoom * get(tempo).bpm) / 60;
	}

	function positionSecFromRuler(clientX: number, altKey: boolean): number {
		if (!pianoRulerEl) return get(transport).position;
		const rect = pianoRulerEl.getBoundingClientRect();
		const bpm = get(tempo).bpm;
		const beatLocal = Math.max(0, (clientX - rect.left + scrollLeft) / zoom);
		const positionSec = clipStartSec + (beatLocal * 60) / bpm;
		return snapPosition(
			positionSec,
			altKey,
			get(tempo),
			timelineScale(),
			get(midiGridBeats),
			get(engineProject).tracks
		);
	}

	function handleRulerMouseDown(event: CustomEvent<MouseEvent>) {
		if (!pianoRulerEl) return;
		scrubCleanup?.();
		scrubCleanup = startTransportScrub(event.detail, positionSecFromRuler, (positionSec) => {
			scrubCleanup = null;
			seek(positionSec).catch(console.error);
		});
	}

	// ── Zoom ──────────────────────────────────────────────────────────────────

	function zoomIn() {
		pianoRollStore.setZoom(zoom * 1.3);
		tick().then(() => {
			resizeCanvases();
			scheduleGridRedraw();
			scheduleNotesRedraw();
		});
	}
	function zoomOut() {
		pianoRollStore.setZoom(zoom / 1.3);
		tick().then(() => {
			resizeCanvases();
			scheduleGridRedraw();
			scheduleNotesRedraw();
		});
	}

	function onGridWheel(e: WheelEvent) {
		if (!editMode) return;
		if (e.ctrlKey || e.metaKey) {
			e.preventDefault();
			if (e.deltaY < 0) zoomIn();
			else zoomOut();
		}
	}

	// ── Réactivité ────────────────────────────────────────────────────────────
</script>

{#if open}
	<div
		class="piano-roll"
		class:piano-roll--preview={!editMode}
		class:piano-roll--resizing={$previewPanelResizing}
		role="application"
		aria-label={$t('pianoRoll.aria')}
	>
		<PianoRollToolbar
			{editMode}
			{mode}
			{quantize}
			{trackColor}
			on:setMode={(e) => pianoRollStore.setMode(e.detail)}
			on:quantizeChange={(e) => applyQuantize(e.detail)}
			on:close={closeEditor}
			on:edit={enterEditFromPreview}
		/>

		<!-- Corps principal -->
		<div class="body" class:body--preview={!editMode}>
			{#if editMode}
				<!-- Piano keys -->
				<PianoRollKeys
					bind:keysEl={keysContainer}
					{controlLanes}
					{laneHeight}
					{laneCandidates}
					{showAddRow}
					{hasLaneData}
					on:switchLane={(e) => switchLane(e.detail.index, e.detail.key)}
					on:addLane={(e) => addLane(e.detail)}
					on:hideLane={(e) => hideLane(e.detail)}
					on:clearLane={(e) => clearLane(e.detail)}
				/>
			{/if}

			<!-- Zone grille + velocity -->
			<div class="grid-wrapper" class:grid-wrapper--preview={!editMode}>
				<!-- Règle beats -->
				<div class="ruler-wrap" style="height: {RULER_HEIGHT}px">
					<TimeRuler
						bind:rulerEl={pianoRulerEl}
						pxPerBeat={zoom}
						numerator={$tempo.numerator}
						scrollPx={scrollLeft}
						viewportWidthPx={rulerViewportWidth}
						maxBars={rulerMaxBars}
						on:mousedown={handleRulerMouseDown}
					/>
				</div>

				<!-- Grid scrollable + scrollbar vertical -->
				<div class="grid-and-vscroll">
					<div
						id="piano-roll-grid"
						class="grid-scroll"
						class:grid-scroll--edit={editMode}
						class:grid-scroll--preview={!editMode}
						bind:this={scrollContainer}
						on:scroll={onGridScroll}
						on:wheel={onGridWheel}
					>
						<div class="grid-stack">
							<canvas
								class="grid-canvas"
								bind:this={canvasGrid}
								style="display:block"
								on:mousedown={onGridMousedown}
								on:mousemove={onGridMousemove}
								on:mouseup={onGridMouseup}
								on:mouseleave={onGridMouseleave}
								on:contextmenu={onGridContextmenu}
							></canvas>
						</div>
					</div>
					{#if editMode}
						<ZoomScrollbar
							orientation="v"
							controls="piano-roll-grid"
							start={vScrollStart}
							end={vScrollEnd}
							on:change={onVScrollChange}
						/>
					{/if}
				</div>

				<!-- Velocity lane (édition uniquement) -->
				{#if editMode}
					<div class="vel-wrapper" bind:this={velWrapper}>
						<canvas
							bind:this={canvasVel}
							on:mousedown={onVelMousedown}
							on:mousemove={onVelMousemove}
							on:mouseup={onVelMouseup}
							style="display:block"
						></canvas>
					</div>
					{#if controlLanes.length > 0 || showAddRow}
						<PianoRollControlLanes
							lanes={controlLanes}
							bind:laneHeight
							{showAddRow}
							{scrollLeft}
							controls={clipControls}
							{expressionNote}
							pxPerBeat={zoom}
							widthPx={lanesWidthPx}
							color={trackColor}
							{quantize}
							commit={commitControls}
							{commitNote}
						/>
					{/if}
					<!-- Scrollbar horizontale avec poignées de zoom -->
					<ZoomScrollbar
						orientation="h"
						controls="piano-roll-grid"
						start={hScrollStart}
						end={hScrollEnd}
						on:change={onHScrollChange}
					/>
				{/if}

				{#if editMode}
					<div class="playhead-clip">
						<Playhead x={playheadMarkerX} />
					</div>
				{:else}
					<Playhead x={playheadMarkerX} />
				{/if}
			</div>
		</div>
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.piano-roll {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: $background-color;
		user-select: none;
		overflow: hidden;
	}

	// ── Corps ─────────────────────────────────────────────────────────────────

	.body {
		display: flex;
		flex: 1;
		overflow: hidden;
	}

	// ── Grille ────────────────────────────────────────────────────────────────

	.grid-wrapper {
		flex: 1;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		min-width: 0;
		position: relative;
	}

	.ruler-wrap {
		position: relative;
		flex-shrink: 0;
	}

	.grid-stack {
		position: relative;
		display: inline-block;
	}

	.grid-canvas {
		display: block;
	}

	.piano-roll--preview .grid-canvas {
		cursor: pointer;
	}

	.grid-and-vscroll {
		flex: 1;
		display: flex;
		flex-direction: row;
		overflow: hidden;
		min-height: 0;
	}

	.playhead-clip {
		position: absolute;
		top: 0;
		left: 0;
		right: 8px; /* exclut la scrollbar verticale */
		bottom: 8px; /* exclut la scrollbar horizontale */
		pointer-events: none;
		overflow: hidden;
	}

	.grid-scroll {
		flex: 1;
		overflow: auto;
		position: relative;
	}

	.grid-scroll--edit {
		overflow: scroll;
		scrollbar-width: none;
		&::-webkit-scrollbar {
			display: none;
		}
	}

	.grid-wrapper--preview {
		width: 100%;
	}

	.grid-scroll--preview {
		overflow: hidden;
		cursor: pointer;
		display: flex;
		align-items: flex-start;
	}

	.piano-roll--resizing {
		.grid-stack {
			contain: layout style paint;
		}
	}

	// ── Velocity lane ─────────────────────────────────────────────────────────

	.vel-wrapper {
		height: 72px;
		flex-shrink: 0;
		overflow-x: auto;
		overflow-y: hidden;
		cursor: ns-resize;
		scrollbar-width: none;
		&::-webkit-scrollbar {
			display: none;
		}
	}
</style>
