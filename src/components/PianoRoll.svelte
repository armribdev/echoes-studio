<!-- PianoRoll.svelte — éditeur MIDI intégré -->
<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { get } from 'svelte/store';
	import { pianoRollStore, pianoRollNotes, type PianoRollMode } from '../stores/pianoRoll';
	import { transport } from '../stores/transport';
	import { tempo } from '../stores/tempo';
	import type { MidiNote } from '../types/global';
	import { engineProject } from '../frontend/store/engineStore';
	import { setMidiNotes, undoCommand, redoCommand } from '../frontend/commands/dispatcher';

	// ── Constantes de rendu ────────────────────────────────────────────────────
	const NOTE_H        = 16;
	const TOTAL_PITCHES = 128;
	const KEYS_WIDTH    = 64;
	const RULER_H       = 28;
	const VEL_H         = 72;
	const VEL_BAR_MIN   = 3;
	const RESIZE_ZONE   = 8;
	const MIN_DURATION  = 0.0625;
	const DEFAULT_ZOOM  = 80;

	const NOTE_NAMES = ['C','C#','D','D#','E','F','F#','G','G#','A','A#','B'];
	const BLACK_KEYS = new Set([1,3,6,8,10]);
	const dpr = typeof window !== 'undefined' ? (window.devicePixelRatio || 1) : 1;

	const QUANTIZE_OPTIONS: { label: string; beats: number }[] = [
		{ label: '1/1',  beats: 4     },
		{ label: '1/2',  beats: 2     },
		{ label: '1/4',  beats: 1     },
		{ label: '1/8',  beats: 0.5   },
		{ label: '1/16', beats: 0.25  },
		{ label: '1/32', beats: 0.125 },
	];

	// ── État local ─────────────────────────────────────────────────────────────
	let canvasGrid: HTMLCanvasElement;
	let canvasVel:  HTMLCanvasElement;
	let scrollContainer: HTMLDivElement;
	let keysContainer:   HTMLDivElement;

	let scrollLeft = 0;
	let scrollTop  = 0;

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

	async function commitNotes() {
		const trackId = get(pianoRollStore).trackId;
		if (!trackId) return;
		try {
			await setMidiNotes(trackId, pianoRollStore.getNotes());
		} catch (e) {
			console.error('commitNotes failed:', e);
		}
	}

	let zoom: number;
	let quantize: number;
	let mode: PianoRollMode;
	let open: boolean;
	let trackColor: string;
	let trackName: string;

	// Doit être déclaré avant les souscriptions (temporal dead zone)
	let rafPending = false;
	let rulerRafPending = false;

	const unsubStore = pianoRollStore.subscribe((s) => {
		zoom       = s.zoom;
		quantize   = s.quantize;
		mode       = s.mode;
		open       = s.open;
		trackColor = s.trackColor;
		trackName  = s.trackName;
	});
	onDestroy(unsubStore);

	let notes: MidiNote[] = [];
	const unsubNotes = pianoRollNotes.subscribe((n) => {
		notes = n;
		scheduleRedraw();
	});
	onDestroy(unsubNotes);

	// Sync notes from Rust state (after undo/redo or external MIDI_NOTES_SET) when not dragging.
	const unsubEngine = engineProject.subscribe(($project) => {
		if (isDragging) return;
		const trackId = get(pianoRollStore).trackId;
		if (!trackId) return;
		const track = $project.tracks.find((t) => t.id === trackId);
		if (track) pianoRollStore.syncNotes(track.midi_notes);
	});
	onDestroy(unsubEngine);

	let playheadBeat = 0;
	const unsubTransport = transport.subscribe((t) => {
		const bpm = get(tempo).bpm;
		playheadBeat = (t.position / 60) * bpm;
		scheduleRedraw();
		scheduleRulerRedraw();
	});
	onDestroy(unsubTransport);

	// ── Cycle de vie ──────────────────────────────────────────────────────────

	function scheduleRedraw() {
		if (rafPending || !open) return;
		rafPending = true;
		requestAnimationFrame(() => {
			rafPending = false;
			drawGrid();
			drawVelocity();
		});
	}

	function scheduleRulerRedraw() {
		if (rulerRafPending || !open) return;
		rulerRafPending = true;
		requestAnimationFrame(() => {
			rulerRafPending = false;
			drawRuler();
		});
	}

	onMount(() => {
		initCanvases();
		scrollToPitch(60);
	});

	$: if (open) {
		tick().then(() => {
			initCanvases();
			scrollToPitch(60);
		});
	}

	function initCanvases() {
		resizeCanvases();
		resizeRuler();
		scheduleRedraw();
		scheduleRulerRedraw();
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
		if (!canvasGrid || !canvasVel || !scrollContainer) return;
		const totalBeats = visibleBeats();
		const totalH     = TOTAL_PITCHES * NOTE_H;
		const w          = Math.max(scrollContainer.clientWidth, totalBeats * zoom + 400);
		setSize(canvasGrid, w, totalH);
		setSize(canvasVel,  w, VEL_H);
	}

	// ── Conversion coordonnées ────────────────────────────────────────────────

	function pitchToY(pitch: number): number  { return (TOTAL_PITCHES - 1 - pitch) * NOTE_H; }
	function snapBeat(beat: number): number    { return Math.round(beat / quantize) * quantize; }

	function visibleBeats(): number {
		const bpm = get(tempo).bpm;
		return Math.max(16, (bpm / 60) * 120);
	}

	function scrollToPitch(pitch: number) {
		if (!scrollContainer) return;
		const y = pitchToY(pitch) - scrollContainer.clientHeight / 2 + NOTE_H / 2;
		scrollContainer.scrollTop = Math.max(0, y);
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

	// ── Dessin de la grille ───────────────────────────────────────────────────

	function drawGrid() {
		if (!canvasGrid) return;
		const ctx = getCtx(canvasGrid);
		if (!ctx) return;

		const W   = canvasGrid.width  / dpr;
		const H   = canvasGrid.height / dpr;
		const sel = get(pianoRollStore).selectedNoteIds;

		ctx.clearRect(0, 0, W, H);

		// ── Fond demi-tons ──────────────────────────────────────────────────
		for (let p = 0; p < TOTAL_PITCHES; p++) {
			const y   = pitchToY(p);
			const isB = BLACK_KEYS.has(p % 12);
			ctx.fillStyle = isB ? '#1b1b22' : '#252530';
			ctx.fillRect(0, y, W, NOTE_H);
		}

		// Surbrillance douce sur C (repère d'octave)
		ctx.fillStyle = 'rgba(255,255,255,0.025)';
		for (let p = 0; p < TOTAL_PITCHES; p += 12) {
			ctx.fillRect(0, pitchToY(p), W, NOTE_H);
		}

		// ── Lignes horizontales ─────────────────────────────────────────────
		ctx.lineWidth = 0.5;
		for (let p = 0; p <= TOTAL_PITCHES; p++) {
			const y = pitchToY(p);
			ctx.strokeStyle = (p % 12 === 0) ? '#38384a' : '#28282e';
			ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(W, y); ctx.stroke();
		}

		// ── Grille verticale ────────────────────────────────────────────────
		const beatsPerMeasure = get(tempo).numerator || 4;
		const totalBeats      = W / zoom;
		for (let b = 0; b <= totalBeats; b += quantize) {
			const x         = b * zoom;
			const isMeasure = Math.abs(b % beatsPerMeasure) < 0.001;
			const isBeat    = Math.abs(b % 1) < 0.001;
			ctx.strokeStyle = isMeasure ? '#3e3e50' : isBeat ? '#2e2e3c' : '#252530';
			ctx.lineWidth   = isMeasure ? 1 : 0.5;
			ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, H); ctx.stroke();
		}

		// ── Playhead ────────────────────────────────────────────────────────
		const phX = playheadBeat * zoom;
		if (phX >= 0 && phX <= W) {
			ctx.save();
			ctx.shadowColor = 'rgba(21, 113, 255, 0.45)';
			ctx.shadowBlur  = 8;
			ctx.strokeStyle = '#1571ff';
			ctx.lineWidth   = 1.5;
			ctx.beginPath(); ctx.moveTo(phX, 0); ctx.lineTo(phX, H); ctx.stroke();
			ctx.restore();
		}

		// ── Ghost note (mode draw, idle) ─────────────────────────────────────
		if (mode === 'draw' && drag.type === 'none' && hoverPitch >= 0 && hoverBeat >= 0) {
			const gBeat = snapBeat(hoverBeat);
			const gx    = gBeat * zoom;
			const gy    = pitchToY(hoverPitch);
			const gw    = Math.max(2, quantize * zoom - 1);
			ctx.fillStyle   = hexAlpha(trackColor, 0.28);
			roundRect(ctx, gx, gy + 1, gw, NOTE_H - 2, 3);
			ctx.fill();
			ctx.strokeStyle = hexAlpha(trackColor, 0.55);
			ctx.lineWidth   = 1;
			roundRect(ctx, gx + 0.5, gy + 1.5, gw - 1, NOTE_H - 3, 3);
			ctx.stroke();
		}

		// ── Notes ───────────────────────────────────────────────────────────
		ctx.font = '9px Inter, system-ui, sans-serif';
		ctx.textBaseline = 'middle';

		for (const note of notes) {
			const x        = note.start * zoom;
			const y        = pitchToY(note.pitch);
			const w        = Math.max(2, note.duration * zoom - 1);
			const selected = sel.has(note.id);

			// Opacité pilotée par la velocity (retour visuel de dynamique)
			const velRatio = note.velocity / 127;
			const alpha    = selected ? 1 : 0.42 + velRatio * 0.52;
			const base     = selected ? lighten(trackColor, 0.22) : trackColor;

			// Corps
			ctx.fillStyle = hexAlpha(base, alpha);
			roundRect(ctx, x, y + 1, w, NOTE_H - 2, 3);
			ctx.fill();

			// Reflet lumineux (gradient vertical)
			if (w > 4) {
				const gr = ctx.createLinearGradient(0, y + 1, 0, y + NOTE_H - 2);
				gr.addColorStop(0,   'rgba(255,255,255,0.20)');
				gr.addColorStop(0.45,'rgba(255,255,255,0.04)');
				gr.addColorStop(1,   'rgba(0,0,0,0.06)');
				ctx.fillStyle = gr;
				roundRect(ctx, x, y + 1, w, NOTE_H - 2, 3);
				ctx.fill();
			}

			// Contour de sélection
			if (selected) {
				ctx.strokeStyle = '#1571ff';
				ctx.lineWidth   = 1.5;
				roundRect(ctx, x + 0.75, y + 1.75, w - 1.5, NOTE_H - 3.5, 2.5);
				ctx.stroke();
			}

			// Label pitch
			if (w > 16) {
				const name  = NOTE_NAMES[note.pitch % 12];
				const oct   = Math.floor(note.pitch / 12) - 1;
				const label = w > 34 ? `${name}${oct}` : name;
				ctx.fillStyle = `rgba(255,255,255,${selected ? 0.9 : 0.5})`;
				ctx.fillText(label, x + 3, y + NOTE_H / 2);
			}

			// Handle resize (triangle coin bas-droit)
			if (w > 10) {
				ctx.fillStyle = `rgba(255,255,255,${selected ? 0.55 : 0.22})`;
				ctx.beginPath();
				ctx.moveTo(x + w - 1, y + NOTE_H - 5);
				ctx.lineTo(x + w - 1, y + NOTE_H - 2);
				ctx.lineTo(x + w - 7, y + NOTE_H - 2);
				ctx.closePath();
				ctx.fill();
			}
		}

		// ── Boîte de sélection ──────────────────────────────────────────────
		if (drag.type === 'selbox') {
			const { x0, y0, x1, y1 } = drag;
			const rx = Math.min(x0, x1);
			const ry = Math.min(y0, y1);
			const rw = Math.abs(x1 - x0);
			const rh = Math.abs(y1 - y0);
			ctx.fillStyle = 'rgba(21, 113, 255, 0.07)';
			ctx.fillRect(rx, ry, rw, rh);
			ctx.strokeStyle = 'rgba(21, 113, 255, 0.65)';
			ctx.lineWidth   = 1;
			ctx.setLineDash([4, 3]);
			ctx.strokeRect(rx + 0.5, ry + 0.5, rw - 1, rh - 1);
			ctx.setLineDash([]);
		}
	}

	function drawVelocity() {
		if (!canvasVel) return;
		const ctx = getCtx(canvasVel);
		if (!ctx) return;
		const W   = canvasVel.width / dpr;
		const H   = VEL_H;
		const sel = get(pianoRollStore).selectedNoteIds;

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
		for (const note of notes) {
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
		const rect = canvasGrid.getBoundingClientRect();
		const rawX = e.clientX - rect.left;
		const rawY = e.clientY - rect.top;
		return {
			beat:  rawX / zoom,
			pitch: TOTAL_PITCHES - 1 - Math.floor(rawY / NOTE_H),
			rawX,
			rawY,
		};
	}

	function hitTest(beat: number, pitch: number): MidiNote | undefined {
		return notes.find(
			(n) => n.pitch === pitch && beat >= n.start && beat < n.start + n.duration
		);
	}

	function isResizeHandle(note: MidiNote, beatX: number): boolean {
		const endBeat = note.start + note.duration;
		const endPx   = endBeat * zoom;
		const pxX     = beatX * zoom;
		return pxX >= endPx - RESIZE_ZONE && pxX <= endPx + 2;
	}

	function onGridMousedown(e: MouseEvent) {
		// Middle-click : pan horizontal
		if (e.button === 1) {
			e.preventDefault();
			isPanning          = true;
			panStartX          = e.clientX;
			panStartScrollLeft = scrollContainer.scrollLeft;
			return;
		}
		if (e.button !== 0 && e.button !== 2) return;
		e.preventDefault();
		isDragging = true;
		const { beat, pitch, rawX, rawY } = getGridCoords(e);
		const hit = hitTest(beat, pitch);

		if (mode === 'draw') {
			if (hit) {
				if (isResizeHandle(hit, beat)) {
					drag = { type: 'resize', noteId: hit.id, origDur: hit.duration, anchorBeat: beat };
				} else {
					drag = { type: 'move', noteId: hit.id, origPitch: hit.pitch, origStart: hit.start, anchorBeat: beat, anchorPitch: pitch, isGroup: false };
				}
			} else {
				const sb = snapBeat(beat);
				const id = pianoRollStore.addNote(pitch, sb, quantize);
				drag = { type: 'create', noteId: id, pitch, startBeat: sb, anchorBeat: beat };
			}
		} else if (mode === 'select') {
			if (hit) {
				if (isResizeHandle(hit, beat)) {
					drag = { type: 'resize', noteId: hit.id, origDur: hit.duration, anchorBeat: beat };
				} else {
					const isSelected = get(pianoRollStore).selectedNoteIds.has(hit.id);
					if (!isSelected) pianoRollStore.selectNote(hit.id, e.ctrlKey || e.metaKey);
					const grp = !(e.ctrlKey || e.metaKey) && get(pianoRollStore).selectedNoteIds.size > 1;
					drag = { type: 'move', noteId: hit.id, origPitch: hit.pitch, origStart: hit.start, anchorBeat: beat, anchorPitch: pitch, isGroup: grp };
				}
			} else {
				if (!(e.ctrlKey || e.metaKey)) pianoRollStore.clearSelection();
				drag = { type: 'selbox', x0: rawX, y0: rawY, x1: rawX, y1: rawY };
			}
		} else if (mode === 'erase') {
			if (hit) pianoRollStore.deleteNote(hit.id);
			drag = { type: 'erase' };
		}
	}

	function onGridMousemove(e: MouseEvent) {
		const { beat, pitch, rawX, rawY } = getGridCoords(e);

		// Mise à jour hover pour ghost note + indicateur de position
		hoverBeat  = beat;
		hoverPitch = Math.max(0, Math.min(127, pitch));

		if (drag.type === 'none') {
			updateCursor(e);
			scheduleRedraw();
			return;
		}

		if (drag.type === 'create') {
			const newDur = Math.max(MIN_DURATION, snapBeat(beat) - drag.startBeat + quantize);
			pianoRollStore.resizeNote(drag.noteId, newDur);
		} else if (drag.type === 'move') {
			if (drag.isGroup) {
				const note = notes.find((n) => n.id === drag.noteId);
				if (note) {
					pianoRollStore.moveSelectedNotes(
						pitch - drag.anchorPitch,
						Math.max(-note.start, snapBeat(beat - drag.anchorBeat))
					);
					drag = { ...drag, anchorBeat: beat, anchorPitch: pitch };
				}
			} else {
				const newStart = Math.max(0, snapBeat(beat - (drag.anchorBeat - drag.origStart)));
				const newPitch = Math.max(0, Math.min(127, drag.origPitch + (pitch - drag.anchorPitch)));
				pianoRollStore.moveNote(drag.noteId, newPitch, newStart);
			}
		} else if (drag.type === 'resize') {
			const newDur = Math.max(MIN_DURATION, snapBeat(beat - drag.anchorBeat) + drag.origDur);
			pianoRollStore.resizeNote(drag.noteId, newDur);
		} else if (drag.type === 'erase') {
			const hit = hitTest(beat, pitch);
			if (hit) pianoRollStore.deleteNote(hit.id);
		} else if (drag.type === 'selbox') {
			drag = { ...drag, x1: rawX, y1: rawY };
			const bLeft  = Math.min(drag.x0, drag.x1) / zoom;
			const bRight = Math.max(drag.x0, drag.x1) / zoom;
			const pTop   = TOTAL_PITCHES - 1 - Math.floor(Math.min(drag.y0, drag.y1) / NOTE_H);
			const pBot   = TOTAL_PITCHES - 1 - Math.floor(Math.max(drag.y0, drag.y1) / NOTE_H);
			const inBox  = notes
				.filter((n) => n.pitch >= pBot && n.pitch <= pTop && n.start < bRight && n.start + n.duration > bLeft)
				.map((n) => n.id);
			pianoRollStore.selectNotes(inBox);
			scheduleRedraw();
		}
	}

	function onGridMouseup(e: MouseEvent) {
		if (drag.type === 'move' && !drag.isGroup) {
			pianoRollStore.selectNote(drag.noteId, false);
		}
		const needsCommit = drag.type === 'create' || drag.type === 'move' || drag.type === 'resize' || drag.type === 'erase';
		drag = { type: 'none' };
		isDragging = false;
		if (needsCommit) commitNotes();
		updateCursor(e);
	}

	function onGridMouseleave() {
		hoverBeat  = -1;
		hoverPitch = -1;
		scheduleRedraw();
	}

	function updateCursor(e: MouseEvent) {
		if (!canvasGrid) return;
		if (isPanning) { canvasGrid.style.cursor = 'grabbing'; return; }
		const { beat, pitch } = getGridCoords(e);
		const hit = hitTest(beat, pitch);
		if (mode === 'draw') {
			canvasGrid.style.cursor = hit
				? (isResizeHandle(hit, beat) ? 'ew-resize' : 'grab')
				: 'crosshair';
		} else if (mode === 'select') {
			canvasGrid.style.cursor = hit
				? (isResizeHandle(hit, beat) ? 'ew-resize' : 'grab')
				: 'default';
		} else {
			canvasGrid.style.cursor = 'cell';
		}
	}

	function onGridContextmenu(e: MouseEvent) {
		e.preventDefault();
		const { beat, pitch } = getGridCoords(e);
		const hit = hitTest(beat, pitch);
		if (hit) { pianoRollStore.deleteNote(hit.id); commitNotes(); }
	}

	// ── Velocity interactions ──────────────────────────────────────────────────

	function updateVelocity(e: MouseEvent) {
		const rect = canvasVel.getBoundingClientRect();
		const rawX = e.clientX - rect.left;
		const beat = rawX / zoom;
		const hit  = notes.find((n) => beat >= n.start && beat < n.start + n.duration);
		if (!hit) return;
		const vel = Math.max(1, Math.min(127,
			Math.round(((VEL_H - (e.clientY - rect.top)) / VEL_H) * 127)
		));
		pianoRollStore.setVelocity(hit.id, vel);
	}

	function onVelMousedown(e: MouseEvent) {
		if (e.button !== 0) return;
		velDragActive = true;
		updateVelocity(e);
	}

	function onVelMousemove(e: MouseEvent) {
		if (!velDragActive) return;
		updateVelocity(e);
	}

	function onVelMouseup() { velDragActive = false; commitNotes(); }

	// ── Scroll synchronisé ────────────────────────────────────────────────────

	function onGridScroll(e: Event) {
		const t   = e.currentTarget as HTMLDivElement;
		scrollLeft = t.scrollLeft;
		scrollTop  = t.scrollTop;
		if (keysContainer) keysContainer.scrollTop = scrollTop;
		scheduleRedraw();
		scheduleRulerRedraw();
	}

	// ── Keyboard ─────────────────────────────────────────────────────────────

	function onKeydown(e: KeyboardEvent) {
		if (!open) return;
		const tag = (e.target as HTMLElement)?.tagName;
		if (tag === 'INPUT' || tag === 'SELECT') return;

		if      (e.key === 'd' || e.key === 'D') pianoRollStore.setMode('draw');
		else if (e.key === 'v' || e.key === 'V' || e.key === 's' || e.key === 'S') pianoRollStore.setMode('select');
		else if (e.key === 'e' || e.key === 'E') pianoRollStore.setMode('erase');
		else if (e.key === 'Escape') {
			const hasSel = get(pianoRollStore).selectedNoteIds.size > 0;
			if (hasSel) pianoRollStore.clearSelection();
			else pianoRollStore.closePianoRoll();
		}
		else if ((e.key === 'Delete' || e.key === 'Backspace') && tag !== 'INPUT') {
			e.preventDefault();
			pianoRollStore.deleteSelectedNotes();
			commitNotes();
		}
		else if ((e.ctrlKey || e.metaKey) && e.key === 'a') { e.preventDefault(); pianoRollStore.selectAll(); }
		else if ((e.ctrlKey || e.metaKey) && e.key === 'z') {
			e.preventDefault();
			if (e.shiftKey) redoCommand(); else undoCommand();
		}
		else if ((e.ctrlKey || e.metaKey) && e.key === 'y') { e.preventDefault(); redoCommand(); }
	}

	function onWindowMousemove(e: MouseEvent) {
		if (isPanning && scrollContainer) {
			const dx = e.clientX - panStartX;
			scrollContainer.scrollLeft = Math.max(0, panStartScrollLeft - dx);
		}
	}

	function onWindowMouseup() {
		if (isPanning) isPanning = false;
		const needsCommit = drag.type === 'create' || drag.type === 'move' || drag.type === 'resize' || drag.type === 'erase';
		if (drag.type !== 'none') { drag = { type: 'none' }; isDragging = false; }
		if (velDragActive) { velDragActive = false; commitNotes(); }
		if (needsCommit) commitNotes();
	}

	onMount(() => {
		window.addEventListener('keydown',    onKeydown);
		window.addEventListener('mousemove',  onWindowMousemove);
		window.addEventListener('mouseup',    onWindowMouseup);
	});
	onDestroy(() => {
		window.removeEventListener('keydown',   onKeydown);
		window.removeEventListener('mousemove', onWindowMousemove);
		window.removeEventListener('mouseup',   onWindowMouseup);
	});

	// ── Règle des beats ───────────────────────────────────────────────────────

	let rulerCanvas: HTMLCanvasElement;

	function drawRuler() {
		if (!rulerCanvas) return;
		const ctx = getCtx(rulerCanvas);
		if (!ctx) return;
		const W   = rulerCanvas.width  / dpr;
		const H   = RULER_H;
		const beatsPerMeasure = get(tempo).numerator || 4;
		const startBeat       = scrollLeft / zoom;
		const endBeat         = startBeat + W / zoom;

		ctx.clearRect(0, 0, W, H);

		// Fond dégradé
		const bg = ctx.createLinearGradient(0, 0, 0, H);
		bg.addColorStop(0, '#21212c');
		bg.addColorStop(1, '#1c1c26');
		ctx.fillStyle = bg;
		ctx.fillRect(0, 0, W, H);

		ctx.font = '10px Inter, system-ui, sans-serif';
		ctx.textBaseline = 'middle';

		for (let b = Math.floor(startBeat * 4) / 4; b <= endBeat; b += 0.25) {
			const x         = b * zoom - scrollLeft;
			if (x < -20 || x > W + 20) continue;
			const isMeasure  = Math.abs(b % beatsPerMeasure) < 0.001;
			const isBeat     = Math.abs(b % 1) < 0.001;
			const isHalf     = Math.abs(b % 0.5) < 0.001 && !isBeat;
			if (!isMeasure && !isBeat && !isHalf) continue;

			ctx.strokeStyle = isMeasure ? '#46465a' : isBeat ? '#32323e' : '#27272e';
			ctx.lineWidth   = isMeasure ? 1 : 0.5;
			const tickH     = isMeasure ? H - 3 : isBeat ? H * 0.5 : H * 0.3;
			ctx.beginPath();
			ctx.moveTo(Math.round(x) + 0.5, H - tickH);
			ctx.lineTo(Math.round(x) + 0.5, H - 1);
			ctx.stroke();

			if (isMeasure) {
				const measure = Math.round(b / beatsPerMeasure) + 1;
				ctx.fillStyle = '#777790';
				ctx.fillText(String(measure), x + 4, H * 0.46);
			}
		}

		// Ligne de séparation basse
		ctx.strokeStyle = '#32323e';
		ctx.lineWidth = 1;
		ctx.beginPath(); ctx.moveTo(0, H - 0.5); ctx.lineTo(W, H - 0.5); ctx.stroke();

		// Playhead triangle
		const phX = playheadBeat * zoom - scrollLeft;
		if (phX >= 0 && phX <= W) {
			ctx.fillStyle = '#1571ff';
			ctx.beginPath();
			ctx.moveTo(phX - 5, 3);
			ctx.lineTo(phX + 5, 3);
			ctx.lineTo(phX, H - 3);
			ctx.closePath();
			ctx.fill();
		}
	}

	$: if (rulerCanvas && open) { resizeRuler(); scheduleRulerRedraw(); }
	$: scrollLeft, zoom, playheadBeat, scheduleRulerRedraw();

	function resizeRuler() {
		if (!rulerCanvas || !scrollContainer) return;
		setSize(rulerCanvas, scrollContainer.clientWidth, RULER_H);
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

	// ── Utilitaires couleur ───────────────────────────────────────────────────

	function hexAlpha(hex: string, alpha: number): string {
		const r = parseInt(hex.slice(1, 3), 16);
		const g = parseInt(hex.slice(3, 5), 16);
		const b = parseInt(hex.slice(5, 7), 16);
		return `rgba(${r},${g},${b},${alpha})`;
	}

	function lighten(hex: string, amount: number): string {
		const r = Math.min(255, parseInt(hex.slice(1, 3), 16) + Math.round(amount * 255));
		const g = Math.min(255, parseInt(hex.slice(3, 5), 16) + Math.round(amount * 255));
		const b = Math.min(255, parseInt(hex.slice(5, 7), 16) + Math.round(amount * 255));
		return `rgb(${r},${g},${b})`;
	}

	function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
		if (w < 2 * r) r = w / 2;
		if (h < 2 * r) r = h / 2;
		ctx.beginPath();
		ctx.moveTo(x + r, y);
		ctx.arcTo(x + w, y, x + w, y + h, r);
		ctx.arcTo(x + w, y + h, x, y + h, r);
		ctx.arcTo(x, y + h, x, y, r);
		ctx.arcTo(x, y, x + w, y, r);
		ctx.closePath();
	}

	// ── Zoom ──────────────────────────────────────────────────────────────────

	function zoomIn()  { pianoRollStore.setZoom(zoom * 1.3); tick().then(resizeCanvases); }
	function zoomOut() { pianoRollStore.setZoom(zoom / 1.3); tick().then(resizeCanvases); }

	function onGridWheel(e: WheelEvent) {
		if (e.ctrlKey || e.metaKey) {
			e.preventDefault();
			if (e.deltaY < 0) zoomIn(); else zoomOut();
		}
	}

	// ── Réactivité ────────────────────────────────────────────────────────────

	$: zoom, quantize, notes, scheduleRedraw();
</script>

{#if open}
<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	class="piano-roll"
	role="region"
	aria-label="Piano Roll"
>
	<!-- Toolbar -->
	<div class="toolbar">
		<button class="toolbar-close" on:click={() => pianoRollStore.closePianoRoll()} title="Fermer (Échap)">
			<svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor">
				<path d="M4.22 4.22a.75.75 0 0 1 1.06 0L8 6.94l2.72-2.72a.75.75 0 1 1 1.06 1.06L9.06 8l2.72 2.72a.75.75 0 1 1-1.06 1.06L8 9.06l-2.72 2.72a.75.75 0 0 1-1.06-1.06L6.94 8 4.22 5.28a.75.75 0 0 1 0-1.06z"/>
			</svg>
		</button>

		<div class="toolbar-track" style="--track-color: {trackColor}">
			<span class="track-dot"></span>
			<span class="track-name">{trackName}</span>
		</div>

		<div class="toolbar-sep"></div>

		<!-- Modes -->
		<div class="mode-group" role="group" aria-label="Mode d'édition">
			<button class="mode-btn" class:active={mode === 'draw'}   on:click={() => pianoRollStore.setMode('draw')}   title="Dessiner (D)">
				<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor">
					<path d="M11.5 1.5a2.121 2.121 0 0 1 3 3L5 14H2v-3L11.5 1.5z"/>
				</svg>
				<span>Dessiner</span>
			</button>
			<button class="mode-btn" class:active={mode === 'select'} on:click={() => pianoRollStore.setMode('select')} title="Sélectionner (S)">
				<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor">
					<path d="M3 2l10 6-5 1.5L6.5 14 3 2z"/>
				</svg>
				<span>Sélection</span>
			</button>
			<button class="mode-btn" class:active={mode === 'erase'}  on:click={() => pianoRollStore.setMode('erase')}  title="Effacer (E)">
				<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor">
					<path d="M9.5 2.5L14 7l-7 7H3v-3l6.5-8.5zM2 13h5.5l1-1.5H2V13z"/>
				</svg>
				<span>Effacer</span>
			</button>
		</div>

		<div class="toolbar-sep"></div>

		<!-- Quantize -->
		<span class="toolbar-label">Q</span>
		<select
			id="quantize-sel"
			class="toolbar-select"
			value={quantize}
			on:change={(e) => pianoRollStore.setQuantize(+e.currentTarget.value)}
		>
			{#each QUANTIZE_OPTIONS as opt}
				<option value={opt.beats}>{opt.label}</option>
			{/each}
		</select>

		<div class="toolbar-sep"></div>

		<!-- Zoom -->
		<div class="zoom-group" aria-label="Zoom horizontal">
			<button class="zoom-btn" on:click={zoomOut} title="Dézoomer (Ctrl+Molette)">−</button>
			<span class="zoom-val">{zoomLabel}</span>
			<button class="zoom-btn" on:click={zoomIn}  title="Zoomer (Ctrl+Molette)">+</button>
		</div>

		<div class="toolbar-sep"></div>

		<!-- Undo / Redo -->
		<button class="icon-btn" on:click={undoCommand} title="Annuler (Ctrl+Z)">
			<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor">
				<path d="M4 4H2V1H1v4.5l.5.5H5V5H3.12A5 5 0 1 1 3 8H2a6 6 0 1 0 2-4z"/>
			</svg>
		</button>
		<button class="icon-btn" on:click={redoCommand} title="Rétablir (Ctrl+Y)">
			<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor">
				<path d="M12 4h2V1h1v4.5l-.5.5H11V5h1.88A5 5 0 1 1 13 8h1a6 6 0 1 0-2-4z"/>
			</svg>
		</button>

		<div class="toolbar-spacer"></div>

		{#if posLabel}
			<span class="pos-readout">{posLabel}</span>
			<div class="toolbar-sep"></div>
		{/if}

		<span class="note-count">{notes.length} note{notes.length !== 1 ? 's' : ''}</span>
	</div>

	<!-- Corps principal -->
	<div class="body">
		<!-- Piano keys -->
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<div class="keys-wrapper">
			<div class="keys-ruler-spacer"></div>
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
		</div>

		<!-- Zone grille + velocity -->
		<div class="grid-wrapper">
			<!-- Règle beats -->
			<canvas class="ruler-canvas" bind:this={rulerCanvas}></canvas>

			<!-- Grid scrollable -->
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<div
				class="grid-scroll"
				bind:this={scrollContainer}
				on:scroll={onGridScroll}
				on:wheel={onGridWheel}
				role="presentation"
			>
				<!-- svelte-ignore a11y-no-static-element-interactions -->
				<canvas
					bind:this={canvasGrid}
					on:mousedown={onGridMousedown}
					on:mousemove={onGridMousemove}
					on:mouseup={onGridMouseup}
					on:mouseleave={onGridMouseleave}
					on:contextmenu={onGridContextmenu}
					style="display:block"
				></canvas>
			</div>

			<!-- Velocity lane -->
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<div class="vel-wrapper" role="presentation">
				<canvas
					bind:this={canvasVel}
					on:mousedown={onVelMousedown}
					on:mousemove={onVelMousemove}
					on:mouseup={onVelMouseup}
					style="display:block"
				></canvas>
			</div>
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

	// ── Toolbar ───────────────────────────────────────────────────────────────

	.toolbar {
		display: flex;
		align-items: center;
		gap: 3px;
		padding: 0 8px;
		height: 36px;
		background: #1d1d27;
		border-bottom: 1px solid #2e2e3c;
		flex-shrink: 0;
		overflow-x: auto;
		overflow-y: hidden;
		scrollbar-width: none;
		&::-webkit-scrollbar { display: none; }
	}

	.toolbar-close {
		display: flex; align-items: center; justify-content: center;
		width: 24px; height: 24px; padding: 0;
		background: transparent; border: none;
		color: #888899; cursor: pointer;
		border-radius: 5px; flex-shrink: 0;
		transition: background 0.12s, color 0.12s;
		&:hover  { background: rgba(255,255,255,0.07); color: #f0f0f8; }
		&:active { background: rgba(255,255,255,0.12); }
	}

	.toolbar-track {
		display: flex; align-items: center; gap: 7px; flex-shrink: 0;
	}

	.track-dot {
		width: 8px; height: 8px; border-radius: 50%;
		background: var(--track-color, #4d96ff);
		flex-shrink: 0;
		box-shadow: 0 0 7px var(--track-color, #4d96ff);
	}

	.track-name {
		font-size: 12px; font-weight: 600;
		color: #ededf5; white-space: nowrap; letter-spacing: 0.01em;
	}

	.toolbar-sep {
		width: 1px; height: 18px;
		background: rgba(255,255,255,0.08);
		flex-shrink: 0; margin: 0 3px;
	}

	.toolbar-spacer { flex: 1; }

	.toolbar-label {
		font-size: 10px; font-weight: 700; letter-spacing: 0.07em;
		color: #55556a; white-space: nowrap; flex-shrink: 0;
		text-transform: uppercase;
	}

	.toolbar-select {
		height: 24px; padding: 0 6px;
		background: rgba(255,255,255,0.05);
		border: 1px solid rgba(255,255,255,0.09);
		border-radius: 5px; color: #d8d8e8;
		font-size: 12px; cursor: pointer; flex-shrink: 0;
		transition: border-color 0.12s, background 0.12s;
		&:hover  { background: rgba(255,255,255,0.08); border-color: rgba(255,255,255,0.16); }
		&:focus  { outline: none; border-color: #1571ff; }
	}

	.mode-group {
		display: flex; gap: 2px; flex-shrink: 0;
		background: rgba(0,0,0,0.22); border-radius: 7px; padding: 2px;
	}

	.mode-btn {
		display: flex; align-items: center; gap: 4px;
		height: 22px; padding: 0 8px;
		background: transparent; border: none; border-radius: 5px;
		color: #777788; font-size: 11px; cursor: pointer;
		white-space: nowrap; flex-shrink: 0;
		transition: background 0.12s, color 0.12s;

		&:hover { background: rgba(255,255,255,0.07); color: #d0d0e0; }

		&.active {
			background: rgba(21, 113, 255, 0.18);
			color: #6aadff;
		}
	}

	.zoom-group {
		display: flex; align-items: center; gap: 3px; flex-shrink: 0;
	}

	.zoom-btn {
		width: 22px; height: 22px; padding: 0;
		display: flex; align-items: center; justify-content: center;
		background: rgba(255,255,255,0.05); border: 1px solid rgba(255,255,255,0.09);
		border-radius: 5px; color: #d0d0e0; font-size: 15px; line-height: 1; cursor: pointer;
		transition: background 0.12s;
		&:hover { background: rgba(255,255,255,0.1); }
	}

	.zoom-val {
		font-size: 11px; font-weight: 600; font-variant-numeric: tabular-nums;
		color: #777788; min-width: 32px; text-align: center;
	}

	.icon-btn {
		display: flex; align-items: center; justify-content: center;
		width: 26px; height: 26px; padding: 0;
		background: transparent; border: 1px solid transparent; border-radius: 5px;
		color: #777788; cursor: pointer; flex-shrink: 0;
		transition: background 0.12s, color 0.12s;
		&:hover  { background: rgba(255,255,255,0.07); color: #d0d0e0; }
		&:active { background: rgba(255,255,255,0.12); }
	}

	.pos-readout {
		font-size: 11px; font-weight: 600;
		font-variant-numeric: tabular-nums; letter-spacing: 0.03em;
		color: #9999c0; white-space: nowrap; flex-shrink: 0;
		background: rgba(21, 113, 255, 0.12);
		padding: 2px 7px; border-radius: 4px;
	}

	.note-count {
		font-size: 11px; color: #3e3e52;
		white-space: nowrap; flex-shrink: 0;
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
		height: 28px; // = RULER_H
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

	// ── Grille ────────────────────────────────────────────────────────────────

	.grid-wrapper {
		flex: 1; display: flex; flex-direction: column;
		overflow: hidden; min-width: 0;
	}

	.ruler-canvas {
		display: block; height: 28px; flex-shrink: 0;
	}

	.grid-scroll {
		flex: 1; overflow: auto; position: relative; cursor: crosshair;

		&::-webkit-scrollbar { width: 8px; height: 8px; }
		&::-webkit-scrollbar-track { background: transparent; }
		&::-webkit-scrollbar-thumb {
			background: rgba(255,255,255,0.09); border-radius: 4px;
			&:hover { background: rgba(255,255,255,0.16); }
		}
		&::-webkit-scrollbar-corner { background: transparent; }
	}

	// ── Velocity lane ─────────────────────────────────────────────────────────

	.vel-wrapper {
		height: 72px; flex-shrink: 0;
		overflow: hidden; cursor: ns-resize;
	}
</style>
