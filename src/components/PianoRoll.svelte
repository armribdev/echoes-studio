<!-- PianoRoll.svelte — éditeur MIDI intégré -->
<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { get } from 'svelte/store';
	import {
		pianoRollStore,
		pianoRollNotes,
		pianoRollSelection,
		type PianoRollMode,
	} from '../stores/pianoRoll';
	import { drawGridLayer } from '../frontend/pianoRoll/gridRenderer';
	import { drawNotesLayer } from '../frontend/pianoRoll/notesRenderer';
	import { hexAlpha, lighten } from '../frontend/pianoRoll/colorUtils';
	import {
		NOTE_H,
		TOTAL_PITCHES,
		RESIZE_ZONE,
		snapBeat,
		hitTestNote,
		isResizeHandle,
		visibleBeatsForEdit,
		pitchFromGridY,
	} from '../frontend/pianoRoll/coords';
	import { transport } from '../stores/transport';
	import { tempo } from '../stores/tempo';
	import type { MidiNote } from '../types/global';
	import { engineProject } from '../frontend/store/engineStore';
	import { setClipMidiNotes, undoCommand, redoCommand, seek } from '../frontend/commands/dispatcher';
	import { snapPosition } from '../lib/snapUtils';
	import { MIDI_GRID_OPTIONS } from '../lib/midiGrid';
	import { midiGridBeats } from '../stores/project';
	import { currentProjectMusical, updateProjectMusical } from '../frontend/commands/dispatcher';
	import { startTransportScrub } from '../frontend/transportScrub';
	import { shouldHandlePianoRollShortcuts, hasNoModifiers } from '../frontend/shortcuts/context';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import { closeEditor, enterEditFromPreview } from '../frontend/pianoRoll/clipEditor';
	import { bottomPanel } from '../stores/bottomPanel';
	import {
		getPreviewPitchRange,
		computePreviewPanelHeight,
		computePreviewRowH,
		PREVIEW_CHROME_H,
	} from '../frontend/pianoRoll/previewLayout';
	import { previewPanelResizing, editPanelResizing } from '../frontend/pianoRoll/previewRefit';
	import { fitEditScrollForNotes, pitchRangeScrollTop } from '../frontend/pianoRoll/editScroll';
	import TimeRuler from './TimeRuler.svelte';
	import Playhead from './Playhead.svelte';
	import ZoomScrollbar from './ZoomScrollbar.svelte';
	import { RULER_HEIGHT } from '../frontend/timeRuler/constants';
	const KEYS_WIDTH    = 64;
	const VEL_H         = 72;
	const VEL_BAR_MIN   = 3;
	const MIN_DURATION  = 0.0625;
	const DEFAULT_ZOOM  = 80;

	const NOTE_NAMES = ['C','C#','D','D#','E','F','F#','G','G#','A','A#','B'];
	const BLACK_KEYS = new Set([1,3,6,8,10]);
	const dpr = typeof window !== 'undefined' ? (window.devicePixelRatio || 1) : 1;

	// ── État local ─────────────────────────────────────────────────────────────
	let canvasGrid: HTMLCanvasElement;
	let canvasVel:  HTMLCanvasElement;
	let scrollContainer: HTMLDivElement;
	let velWrapper: HTMLDivElement;
	let keysContainer:   HTMLDivElement;

	let scrollLeft = 0;
	let scrollTop  = 0;

	// ── Scrollbars personnalisés ────────────────────────────────────────────────
	let hScrollStart = 0;
	let hScrollEnd   = 1;
	let vScrollStart = 0;
	let vScrollEnd   = 1;
	let hTotalBeats  = 1; // beats totaux dans le canvas (pour zoom cohérent)

	// Hover (ghost note + position indicator)
	let hoverBeat  = -1;
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
		| { type: 'move';   noteId: string; origPitch: number; origStart: number; anchorBeat: number; anchorPitch: number; isGroup: boolean }
		| { type: 'resize'; noteId: string; origDur: number; anchorBeat: number }
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
			(editMode
				? visibleBeatsForEdit(notes, clipDurationBeats)
				: Math.max(clipDurationBeats, 1)) / ($tempo.numerator || 4),
		) + 4;

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

	// Sync notes from Rust state (after undo/redo or MIDI_CLIP_NOTES_SET) when not dragging.
	const unsubEngine = engineProject.subscribe(($project) => {
		if (isDragging || !get(pianoRollStore).open) return;
		const { trackId, clipId } = get(pianoRollStore);
		if (!trackId || !clipId) return;
		const track = $project.tracks.find((t) => t.id === trackId);
		if (!track) return;
		const clip = track.midi_clips.find((c) => c.id === clipId);
		if (clip) pianoRollStore.syncNotes(clip.notes ?? []);
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

	/** @deprecated alias — notes + velocity */
	function scheduleRedraw() {
		scheduleGridRedraw();
		scheduleNotesRedraw();
		if (editMode) drawVelocity();
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
			maxScrollTop,
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
			scrollContainer.scrollTop,
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
		c.width  = Math.round(w * dpr);
		c.height = Math.round(h * dpr);
		c.style.width  = w + 'px';
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
		const effectivePanelH =
			measuredGridH > 0 ? measuredGridH + PREVIEW_CHROME_H : panelH;
		previewRowH = computePreviewRowH(notes, effectivePanelH);
	}

	async function applyPreviewFit(
		panelH: number,
		opts: { fitZoom?: boolean; resetScroll?: boolean } = {},
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

	// ── Conversion coordonnées ────────────────────────────────────────────────

	function pitchToY(pitch: number): number {
		if (editMode) return (TOTAL_PITCHES - 1 - pitch) * NOTE_H;
		return (previewPitchMax - pitch) * previewRowH;
	}
	function visibleBeats(): number {
		if (!editMode) return Math.max(get(pianoRollStore).clipDurationBeats, 1);
		return visibleBeatsForEdit(displayNotes(), clipDurationBeats);
	}

	// Indicateur de position (toolbar)
	function beatLabel(beat: number): string {
		const num     = get(tempo).numerator || 4;
		const measure = Math.floor(beat / num) + 1;
		const b       = Math.floor(beat % num) + 1;
		return `${measure}:${b}`;
	}

	$: posLabel = hoverPitch >= 0 && hoverBeat >= 0
		? `${NOTE_NAMES[hoverPitch % 12]}${Math.floor(hoverPitch / 12) - 1}  ${beatLabel(hoverBeat)}`
		: '';

	$: zoomLabel = `${(zoom / DEFAULT_ZOOM).toFixed(1)}×`;

	$: clipMetaLabel = (() => {
		const num = $tempo.numerator || 4;
		const bars = clipDurationBeats / num;
		const barsRounded = Math.round(bars * 10) / 10;
		const barsText = Number.isInteger(barsRounded)
			? `${barsRounded}`
			: `${barsRounded.toFixed(1)}`;
		return `${barsText} mes.`;
	})();

	$: noteMetaLabel = `${notes.length} note${notes.length !== 1 ? 's' : ''}`;

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
			previewRowH,
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
			selbox: drag.type === 'selbox' ? drag : undefined,
		});
	}

	function drawVelocity() {
		if (!canvasVel) return;
		const ctx = getCtx(canvasVel);
		if (!ctx) return;
		const W   = canvasVel.width / dpr;
		const H   = VEL_H;
		ctx.clearRect(0, 0, W, H);

		// Fond
		ctx.fillStyle = '#18181f';
		ctx.fillRect(0, 0, W, H);

		// Lignes de repère horizontales
		ctx.lineWidth = 0.5;
		[0.25, 0.5, 0.75].forEach((p) => {
			ctx.strokeStyle = '#252530';
			const y = H - p * (H - 4);
			ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(W, y); ctx.stroke();
		});

		// Barres de velocity
		const velNotes = displayNotes();
		const sel = displaySelection();
		for (const note of velNotes) {
			const x        = note.start * zoom + 1;
			const barH     = Math.max(VEL_BAR_MIN, (note.velocity / 127) * (H - 6));
			const barW     = Math.max(3, quantize * zoom - 3);
			const y        = H - barH;
			const selected = sel.has(note.id);
			const base     = selected ? lighten(trackColor, 0.2) : trackColor;
			const alpha    = selected ? 0.9 : 0.62;

			// Gradient du bas vers le haut
			const gr = ctx.createLinearGradient(0, y, 0, H);
			gr.addColorStop(0, hexAlpha(base, alpha));
			gr.addColorStop(1, hexAlpha(base, alpha * 0.55));
			ctx.fillStyle = gr;

			// Arrondi en haut uniquement
			const r = Math.min(2, barW / 2);
			ctx.beginPath();
			ctx.moveTo(x + r, y);
			ctx.lineTo(x + barW - r, y);
			ctx.quadraticCurveTo(x + barW, y, x + barW, y + r);
			ctx.lineTo(x + barW, H);
			ctx.lineTo(x, H);
			ctx.lineTo(x, y + r);
			ctx.quadraticCurveTo(x, y, x + r, y);
			ctx.closePath();
			ctx.fill();
		}

		// Séparateur haut
		ctx.strokeStyle = '#32323e';
		ctx.lineWidth   = 1;
		ctx.beginPath(); ctx.moveTo(0, 0.5); ctx.lineTo(W, 0.5); ctx.stroke();
	}

	// ── Interactions Canvas ───────────────────────────────────────────────────

	function getGridCoords(e: MouseEvent): { beat: number; pitch: number; rawX: number; rawY: number } {
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
						Math.min(previewPitchMax, previewPitchMax - Math.floor(rawY / rowH)),
					),
			rawX,
			rawY,
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

	function moveWorkingNote(id: string, pitch: number, start: number) {
		if (!workingNotes) return;
		workingNotes = workingNotes.map((n) =>
			n.id === id
				? { ...n, pitch: Math.max(0, Math.min(127, pitch)), start: Math.max(0, start) }
				: n,
		);
		scheduleNotesRedraw();
	}

	function moveWorkingSelected(deltaPitch: number, deltaStart: number) {
		if (!workingNotes) return;
		const sel = get(pianoRollStore).selectedNoteIds;
		workingNotes = workingNotes.map((n) =>
			sel.has(n.id)
				? {
						...n,
						pitch: Math.max(0, Math.min(127, n.pitch + deltaPitch)),
						start: Math.max(0, n.start + deltaStart),
					}
				: n,
		);
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
			isPanning          = true;
			panStartX          = e.clientX;
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
		const { beat, pitch, rawX, rawY } = getGridCoords(e);
		const hit = hitTest(beat, pitch);

		if (mode === 'draw') {
			beginWorkingNotes();
			if (hit) {
				if (noteResizeHandle(hit, beat)) {
					drag = { type: 'resize', noteId: hit.id, origDur: hit.duration, anchorBeat: beat };
				} else {
					drag = { type: 'move', noteId: hit.id, origPitch: hit.pitch, origStart: hit.start, anchorBeat: beat, anchorPitch: pitch, isGroup: false };
				}
			} else {
				const sb = snapBeat(beat, quantize);
				const id = crypto.randomUUID().slice(0, 8);
				workingNotes = [
					...displayNotes(),
					{ id, pitch, start: sb, duration: quantize, velocity: 100 },
				];
				drag = { type: 'create', noteId: id, pitch, startBeat: sb, anchorBeat: beat };
				scheduleNotesRedraw();
				scheduleVelRedraw();
			}
		} else if (mode === 'select') {
			if (hit) {
				beginWorkingNotes();
				if (noteResizeHandle(hit, beat)) {
					drag = { type: 'resize', noteId: hit.id, origDur: hit.duration, anchorBeat: beat };
				} else {
					const isSelected = get(pianoRollStore).selectedNoteIds.has(hit.id);
					if (!isSelected) pianoRollStore.selectNote(hit.id, e.ctrlKey || e.metaKey);
					const grp = !(e.ctrlKey || e.metaKey) && get(pianoRollStore).selectedNoteIds.size > 1;
					drag = { type: 'move', noteId: hit.id, origPitch: hit.pitch, origStart: hit.start, anchorBeat: beat, anchorPitch: pitch, isGroup: grp };
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
			hoverBeat  = beat;
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
				const note = displayNotes().find((n) => n.id === moveDrag.noteId);
				if (note) {
					moveWorkingSelected(
						pitch - moveDrag.anchorPitch,
						Math.max(-note.start, snapBeat(beat - moveDrag.anchorBeat, quantize)),
					);
					drag = { ...moveDrag, anchorBeat: beat, anchorPitch: pitch };
				}
			} else {
				const newStart = Math.max(0, snapBeat(beat - (moveDrag.anchorBeat - moveDrag.origStart), quantize));
				const newPitch = Math.max(0, Math.min(127, moveDrag.origPitch + (pitch - moveDrag.anchorPitch)));
				moveWorkingNote(moveDrag.noteId, newPitch, newStart);
			}
		} else if (drag.type === 'resize') {
			const newDur = Math.max(MIN_DURATION, snapBeat(beat - drag.anchorBeat, quantize) + drag.origDur);
			resizeWorkingNote(drag.noteId, newDur);
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
						n.pitch >= pBot &&
						n.pitch <= pTop &&
						n.start < bRight &&
						n.start + n.duration > bLeft,
				)
				.map((n) => n.id);
			scheduleNotesRedraw();
		}
	}

	function onGridMouseup(e: MouseEvent) {
		if (drag.type === 'move' && !drag.isGroup) {
			pianoRollStore.selectNote(drag.noteId, false);
		}
		const needsCommit =
			drag.type === 'create' ||
			drag.type === 'move' ||
			drag.type === 'resize' ||
			drag.type === 'erase' ||
			drag.type === 'selbox';
		flushWorkingNotes();
		drag = { type: 'none' };
		isDragging = false;
		if (needsCommit) commitNotes();
		updateCursor(e);
	}

	function onGridMouseleave() {
		hoverBeat = -1;
		hoverPitch = -1;
		scheduleHoverRedraw();
	}

	function updateCursor(e: MouseEvent) {
		if (!canvasGrid) return;
		if (isPanning) { canvasGrid.style.cursor = 'grabbing'; return; }
		const { beat, pitch } = getGridCoords(e);
		const hit = hitTest(beat, pitch);
		if (mode === 'draw') {
			canvasGrid.style.cursor = hit
				? (noteResizeHandle(hit, beat) ? 'ew-resize' : 'grab')
				: 'crosshair';
		} else if (mode === 'select') {
			canvasGrid.style.cursor = hit
				? (noteResizeHandle(hit, beat) ? 'ew-resize' : 'grab')
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
		if (hit) { pianoRollStore.deleteNote(hit.id); commitNotes(); }
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
			Math.min(127, Math.round(((VEL_H - (e.clientY - rect.top)) / VEL_H) * 127)),
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
		updateVelocity(e);
	}

	function onVelMousemove(e: MouseEvent) {
		if (!velDragActive) return;
		updateVelocity(e);
	}

	function onVelMouseup() {
		velDragActive = false;
		flushWorkingNotes();
		commitNotes();
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
		const vpW    = scrollContainer.clientWidth;
		const totalH = scrollContainer.scrollHeight;
		const vpH    = scrollContainer.clientHeight;

		hTotalBeats  = zoom > 0 ? totalW / zoom : 1;
		hScrollStart = totalW > vpW ? scrollLeft / totalW : 0;
		hScrollEnd   = totalW > vpW ? Math.min(1, (scrollLeft + vpW) / totalW) : 1;
		vScrollStart = totalH > vpH ? scrollTop  / totalH : 0;
		vScrollEnd   = totalH > vpH ? Math.min(1, (scrollTop  + vpH) / totalH) : 1;
	}

	function onHScrollChange(e: CustomEvent<{ start: number; end: number; cause: string }>) {
		if (!scrollContainer || !editMode) return;
		const { start: ns, end: ne } = e.detail;

		const totalBeats = hTotalBeats;
		const vpW        = scrollContainer.clientWidth;
		const leftBeat   = ns * totalBeats;
		const rightBeat  = ne * totalBeats;
		const spanBeats  = rightBeat - leftBeat;
		if (spanBeats <= 0) return;

		const newZoom      = Math.max(12, Math.min(400, vpW / spanBeats));
		const newScrollLeft = Math.max(0, leftBeat * newZoom);

		// Mise à jour visuelle immédiate pour fluidité
		hScrollStart = ns;
		hScrollEnd   = ne;

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

		const totalH   = scrollContainer.scrollHeight;
		const vpH      = scrollContainer.clientHeight;
		const thumbSz  = vScrollEnd - vScrollStart;

		let newScrollTop: number;

		if (cause === 'resize-start') {
			// Poignée haute : fait défiler vers la position de début
			newScrollTop = ns * totalH;
			vScrollStart = ns;
			vScrollEnd   = Math.min(1, ns + thumbSz);
		} else if (cause === 'resize-end') {
			// Poignée basse : fait défiler pour que la fin soit visible
			newScrollTop = (ne - thumbSz) * totalH;
			vScrollStart = Math.max(0, ne - thumbSz);
			vScrollEnd   = ne;
		} else {
			// Pan
			newScrollTop = ns * totalH;
			vScrollStart = ns;
			vScrollEnd   = ne;
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

		let handled = false;

		if (hasNoModifiers(e)) {
			if      (e.key === 'd' || e.key === 'D') { pianoRollStore.setMode('draw');   handled = true; }
			else if (e.key === 'v' || e.key === 'V' || e.key === 's' || e.key === 'S') { pianoRollStore.setMode('select'); handled = true; }
			else if (e.key === 'e' || e.key === 'E') { pianoRollStore.setMode('erase');  handled = true; }
		}

		if (!handled && (e.key === 'Delete' || e.key === 'Backspace')) {
			e.preventDefault();
			pianoRollStore.deleteSelectedNotes();
			commitNotes();
			handled = true;
		}
		else if (!handled && (e.ctrlKey || e.metaKey) && e.key === 'a') {
			e.preventDefault();
			pianoRollStore.selectAll();
			handled = true;
		}
		else if (!handled && (e.ctrlKey || e.metaKey) && e.key === 'z') {
			e.preventDefault();
			if (e.shiftKey) redoCommand(); else undoCommand();
			handled = true;
		}
		else if (!handled && (e.ctrlKey || e.metaKey) && e.key === 'y') {
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
		const needsCommit =
			drag.type === 'create' ||
			drag.type === 'move' ||
			drag.type === 'resize' ||
			drag.type === 'erase' ||
			drag.type === 'selbox';
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
	}

	onMount(() => {
		window.addEventListener('keydown',    onKeydown);
		window.addEventListener('mousemove',  onWindowMousemove);
		window.addEventListener('mouseup',    onWindowMouseup);
	});
	onDestroy(() => {
		gridViewportObserver?.disconnect();
		scrubCleanup?.();
		window.removeEventListener('keydown',   onKeydown);
		window.removeEventListener('mousemove', onWindowMousemove);
		window.removeEventListener('mouseup',   onWindowMouseup);
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
		return zoom * get(tempo).bpm / 60;
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
			get(engineProject).tracks,
		);
	}

	function handleRulerMouseDown(event: CustomEvent<MouseEvent>) {
		if (!pianoRulerEl) return;
		scrubCleanup?.();
		scrubCleanup = startTransportScrub(
			event.detail,
			positionSecFromRuler,
			(positionSec) => {
				scrubCleanup = null;
				seek(positionSec).catch(console.error);
			},
		);
	}

	// ── Piano keys ────────────────────────────────────────────────────────────

	function getPianoKeyLabel(pitch: number): string {
		const deg = pitch % 12;
		const oct = Math.floor(pitch / 12) - 1;
		if (deg === 0)  return `C${oct}`;
		if (deg === 4)  return `E${oct}`;
		if (deg === 9)  return `A${oct}`;
		return '';
	}

	function isBlack(pitch: number): boolean { return BLACK_KEYS.has(pitch % 12); }

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
			if (e.deltaY < 0) zoomIn(); else zoomOut();
		}
	}

	// ── Réactivité ────────────────────────────────────────────────────────────

</script>

{#if open}
<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
<div
	class="piano-roll"
	class:piano-roll--preview={!editMode}
	class:piano-roll--resizing={$previewPanelResizing}
	role="application"
	aria-label="Piano Roll"
>
	{#if editMode}
	<div class="chrome" style="--track-color: {trackColor}">
		<div class="chrome-inner">
			<div class="mode-group" role="group" aria-label="Mode d'édition">
				<button class="mode-btn" class:active={mode === 'draw'} on:click={() => pianoRollStore.setMode('draw')} title="Dessiner (D)" aria-label="Dessiner">
					<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor" aria-hidden="true">
						<path d="M11.5 1.5a2.121 2.121 0 0 1 3 3L5 14H2v-3L11.5 1.5z"/>
					</svg>
				</button>
				<button class="mode-btn" class:active={mode === 'select'} on:click={() => pianoRollStore.setMode('select')} title="Sélectionner (S)" aria-label="Sélectionner">
					<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor" aria-hidden="true">
						<path d="M3 2l10 6-5 1.5L6.5 14 3 2z"/>
					</svg>
				</button>
				<button class="mode-btn" class:active={mode === 'erase'} on:click={() => pianoRollStore.setMode('erase')} title="Effacer (E)" aria-label="Effacer">
					<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor" aria-hidden="true">
						<path d="M9.5 2.5L14 7l-7 7H3v-3l6.5-8.5zM2 13h5.5l1-1.5H2V13z"/>
					</svg>
				</button>
			</div>

			<div class="chrome-divider"></div>

			<select
				class="q-select"
				value={quantize}
				on:change={(e) => {
					const beats = +e.currentTarget.value;
					pianoRollStore.setQuantize(beats);
					midiGridBeats.set(beats);
					updateProjectMusical({ ...currentProjectMusical(), midi_grid_beats: beats }).catch(
						console.error,
					);
				}}
				title="Quantification (enregistrée dans le projet)"
				aria-label="Quantification"
			>
				{#each MIDI_GRID_OPTIONS as opt}
					<option value={opt.beats}>{opt.label}</option>
				{/each}
			</select>

			<div class="chrome-divider"></div>

			<div class="zoom-group" aria-label="Zoom horizontal">
				<button class="zoom-btn" on:click={zoomOut} title="Dézoomer">−</button>
				<span class="zoom-val">{zoomLabel}</span>
				<button class="zoom-btn" on:click={zoomIn} title="Zoomer">+</button>
			</div>

			<div class="chrome-spacer"></div>

			{#if posLabel}
				<span class="pos-readout">{posLabel}</span>
			{/if}

			<span class="meta-chip">{noteMetaLabel} · {clipMetaLabel}</span>

			<button class="chrome-close" on:click={closeEditor} title="Fermer (Tab)" aria-label="Fermer">
				<svg viewBox="0 0 10 10" width="10" height="10" fill="none" aria-hidden="true">
					<path d="M1.5 1.5L8.5 8.5M8.5 1.5L1.5 8.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
				</svg>
			</button>
		</div>
	</div>
	{:else}
	<div class="chrome chrome--preview" style="--track-color: {trackColor}">
		<div class="chrome-inner">
			<div class="chrome-spacer"></div>
			<span class="meta-chip">{noteMetaLabel} · {clipMetaLabel}</span>
			<button class="edit-btn" on:click={enterEditFromPreview}>
				Éditer
			</button>
			<button class="chrome-close" on:click={closeEditor} title="Fermer (Tab)" aria-label="Fermer">
				<svg viewBox="0 0 10 10" width="10" height="10" fill="none" aria-hidden="true">
					<path d="M1.5 1.5L8.5 8.5M8.5 1.5L1.5 8.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
				</svg>
			</button>
		</div>
	</div>
	{/if}

	<!-- Corps principal -->
	<div class="body" class:body--preview={!editMode}>
		{#if editMode}
		<!-- Piano keys -->
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<div class="keys-wrapper">
			<div class="keys-ruler-spacer" style="height: {RULER_HEIGHT}px"></div>
			<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
			<div class="keys-scroll" bind:this={keysContainer} role="list" aria-label="Touches piano">
				{#each { length: TOTAL_PITCHES } as _, i}
					{@const pitch = TOTAL_PITCHES - 1 - i}
					<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
					<div
						class="key"
						class:key--black={isBlack(pitch)}
						class:key--c={pitch % 12 === 0}
						style="height: {NOTE_H}px"
						role="listitem"
					>
						<span class="key-label">{getPianoKeyLabel(pitch)}</span>
					</div>
				{/each}
			</div>
			<div class="keys-vel-label">VEL</div>
			<div class="keys-hscroll-spacer"></div>
		</div>
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
				<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
				<div
					class="grid-scroll"
					class:grid-scroll--edit={editMode}
					class:grid-scroll--preview={!editMode}
					bind:this={scrollContainer}
					on:scroll={onGridScroll}
					on:wheel={onGridWheel}
				>
					<div class="grid-stack">
						<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
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
					start={vScrollStart}
					end={vScrollEnd}
					on:change={onVScrollChange}
				/>
				{/if}
			</div>

			<!-- Velocity lane (édition uniquement) -->
			{#if editMode}
			<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
			<div class="vel-wrapper" bind:this={velWrapper}>
				<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
				<canvas
					bind:this={canvasVel}
					on:mousedown={onVelMousedown}
					on:mousemove={onVelMousemove}
					on:mouseup={onVelMouseup}
					style="display:block"
				></canvas>
			</div>
			<!-- Scrollbar horizontale avec poignées de zoom -->
			<ZoomScrollbar
				orientation="h"
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
	.piano-roll {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: #1a1a22;
		user-select: none;
		overflow: hidden;
	}

	// ── Chrome (barre unique) ─────────────────────────────────────────────────

	.chrome {
		display: flex;
		flex-shrink: 0;
		height: 30px;
		background: #1d1d27;
		border-bottom: 1px solid #2e2e3c;
		box-shadow: inset 3px 0 0 var(--track-color, #4d96ff);

		&--preview {
			background: #1a1a22;
			border-bottom-color: #282832;
		}
	}

	.chrome-inner {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 0 8px 0 11px;
		overflow-x: auto;
		overflow-y: hidden;
		scrollbar-width: none;
		&::-webkit-scrollbar { display: none; }
	}

	.chrome-divider {
		width: 1px;
		height: 16px;
		background: rgba(255, 255, 255, 0.07);
		flex-shrink: 0;
	}

	.chrome-spacer { flex: 1; min-width: 8px; }

	.chrome-close {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 22px;
		height: 22px;
		padding: 0;
		background: transparent;
		border: none;
		border-radius: 4px;
		color: #666678;
		cursor: pointer;
		flex-shrink: 0;
		transition: background 0.12s, color 0.12s;

		&:hover { background: rgba(255, 65, 54, 0.14); color: #ff6b60; }
	}

	.edit-btn {
		height: 22px;
		padding: 0 10px;
		background: rgba(21, 113, 255, 0.12);
		border: 1px solid rgba(21, 113, 255, 0.28);
		border-radius: 4px;
		color: #6aadff;
		font-size: 11px;
		font-weight: 500;
		cursor: pointer;
		flex-shrink: 0;
		transition: background 0.12s, border-color 0.12s;

		&:hover {
			background: rgba(21, 113, 255, 0.2);
			border-color: rgba(21, 113, 255, 0.45);
		}
	}

	.meta-chip {
		font-size: 10px;
		font-variant-numeric: tabular-nums;
		color: #555568;
		white-space: nowrap;
		flex-shrink: 0;
		letter-spacing: 0.01em;
	}

	.q-select {
		height: 22px;
		padding: 0 6px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.08);
		border-radius: 4px;
		color: #c8c8d8;
		font-size: 11px;
		cursor: pointer;
		flex-shrink: 0;
		transition: border-color 0.12s, background 0.12s;

		&:hover { background: rgba(255, 255, 255, 0.07); border-color: rgba(255, 255, 255, 0.14); }
		&:focus { outline: none; border-color: #1571ff; }
	}

	.mode-group {
		display: flex;
		gap: 1px;
		flex-shrink: 0;
		background: rgba(0, 0, 0, 0.25);
		border-radius: 6px;
		padding: 2px;
	}

	.mode-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 26px;
		height: 22px;
		padding: 0;
		background: transparent;
		border: none;
		border-radius: 4px;
		color: #666678;
		cursor: pointer;
		flex-shrink: 0;
		transition: background 0.12s, color 0.12s;

		&:hover { background: rgba(255, 255, 255, 0.07); color: #d0d0e0; }

		&.active {
			background: rgba(21, 113, 255, 0.2);
			color: #6aadff;
		}
	}

	.zoom-group {
		display: flex;
		align-items: center;
		gap: 2px;
		flex-shrink: 0;
	}

	.zoom-btn {
		width: 20px;
		height: 20px;
		padding: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.08);
		border-radius: 4px;
		color: #b8b8c8;
		font-size: 14px;
		line-height: 1;
		cursor: pointer;
		transition: background 0.12s;

		&:hover { background: rgba(255, 255, 255, 0.09); }
	}

	.zoom-val {
		font-size: 10px;
		font-weight: 600;
		font-variant-numeric: tabular-nums;
		color: #666678;
		min-width: 28px;
		text-align: center;
	}

	.pos-readout {
		font-size: 10px;
		font-weight: 600;
		font-variant-numeric: tabular-nums;
		letter-spacing: 0.02em;
		color: #9090b8;
		white-space: nowrap;
		flex-shrink: 0;
		background: rgba(21, 113, 255, 0.1);
		padding: 2px 7px;
		border-radius: 4px;
	}

	// ── Corps ─────────────────────────────────────────────────────────────────

	.body {
		display: flex;
		flex: 1;
		overflow: hidden;
	}

	// ── Piano keys ────────────────────────────────────────────────────────────

	.keys-wrapper {
		display: flex; flex-direction: column;
		width: 64px; flex-shrink: 0;
		border-right: 1px solid #2a2a38;
	}

	.keys-ruler-spacer {
		background: #1d1d28;
		border-bottom: 1px solid #2a2a38;
		flex-shrink: 0;
	}

	.keys-scroll {
		flex: 1;
		overflow: hidden;
		display: flex; flex-direction: column;
	}

	.key {
		display: flex; align-items: center; justify-content: flex-end;
		padding-right: 5px; box-sizing: border-box; flex-shrink: 0;
		background: #252535; border-bottom: 1px solid #1c1c28;
		cursor: pointer; transition: background 0.07s;

		&:hover { background: #303048; }

		&--black {
			background: #17171e; border-bottom: 1px solid #101015;
			&:hover { background: #21212e; }
		}

		&--c {
			border-bottom: 1px solid #32323e;
		}
	}

	.key-label {
		font-size: 9px; font-weight: 600; letter-spacing: 0.02em;
		color: #4a4a60; pointer-events: none;
	}

	.keys-vel-label {
		height: 72px; // = VEL_H
		flex-shrink: 0;
		border-top: 1px solid #2a2a38;
		display: flex; align-items: center; justify-content: center;
		font-size: 9px; font-weight: 700; letter-spacing: 0.1em;
		color: #383848; text-transform: uppercase;
	}

	.keys-hscroll-spacer {
		height: 8px;
		flex-shrink: 0;
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
		top: 0; left: 0;
		right: 8px;  /* exclut la scrollbar verticale */
		bottom: 8px; /* exclut la scrollbar horizontale */
		pointer-events: none;
		overflow: hidden;
	}

	.grid-scroll {
		flex: 1; overflow: auto; position: relative;
	}

	.grid-scroll--edit {
		overflow: scroll;
		scrollbar-width: none;
		&::-webkit-scrollbar { display: none; }
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
		height: 72px; flex-shrink: 0;
		overflow-x: auto;
		overflow-y: hidden;
		cursor: ns-resize;
		scrollbar-width: none;
		&::-webkit-scrollbar { display: none; }
	}
</style>
