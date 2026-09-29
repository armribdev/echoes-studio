import type { LibraryMidiNote } from './types';

/**
 * Isolated WebAudio MIDI preview (ADR-023 — UI-side, not the engine callback).
 * Simple triangle oscillators; no arrange playhead / clip creation.
 */
class LibraryMidiPreviewPlayer {
	private ctx: AudioContext | null = null;
	private nodes: OscillatorNode[] = [];
	private gains: GainNode[] = [];
	private playingId: string | null = null;
	private generation = 0;
	private endTimer: ReturnType<typeof setTimeout> | null = null;
	private onEndedCb: (() => void) | null = null;

	private ensureContext(): AudioContext {
		if (!this.ctx) this.ctx = new AudioContext();
		return this.ctx;
	}

	setOnEnded(cb: (() => void) | null): void {
		this.onEndedCb = cb;
	}

	isPlaying(id?: string): boolean {
		if (!this.playingId) return false;
		return id == null ? true : this.playingId === id;
	}

	async play(id: string, notes: LibraryMidiNote[], bpm: number): Promise<void> {
		this.stop();
		const gen = ++this.generation;
		const ctx = this.ensureContext();
		if (ctx.state === 'suspended') await ctx.resume();
		if (gen !== this.generation) return;

		const beatSec = 60 / Math.max(1, bpm);
		const now = ctx.currentTime + 0.02;
		let maxEnd = 0;

		for (const note of notes) {
			const start = now + note.start * beatSec;
			const dur = Math.max(0.04, note.duration * beatSec);
			const end = start + dur;
			maxEnd = Math.max(maxEnd, end);

			const osc = ctx.createOscillator();
			const gain = ctx.createGain();
			osc.type = 'triangle';
			osc.frequency.value = midiToHz(note.pitch);
			const peak = (Math.max(1, Math.min(127, note.velocity)) / 127) * 0.18;
			gain.gain.setValueAtTime(0.0001, start);
			gain.gain.exponentialRampToValueAtTime(peak, start + 0.01);
			gain.gain.exponentialRampToValueAtTime(0.0001, end);

			osc.connect(gain);
			gain.connect(ctx.destination);
			osc.start(start);
			osc.stop(end + 0.02);
			this.nodes.push(osc);
			this.gains.push(gain);
		}

		this.playingId = id;
		const ms = Math.max(50, (maxEnd - ctx.currentTime) * 1000 + 30);
		this.endTimer = setTimeout(() => {
			if (gen !== this.generation) return;
			this.cleanupNodes();
			this.playingId = null;
			this.onEndedCb?.();
		}, ms);
	}

	stop(): void {
		this.generation += 1;
		if (this.endTimer != null) {
			clearTimeout(this.endTimer);
			this.endTimer = null;
		}
		this.cleanupNodes();
		this.playingId = null;
	}

	private cleanupNodes() {
		for (const osc of this.nodes) {
			try {
				osc.stop();
			} catch {
				/* already stopped */
			}
			try {
				osc.disconnect();
			} catch {
				/* ignore */
			}
		}
		for (const g of this.gains) {
			try {
				g.disconnect();
			} catch {
				/* ignore */
			}
		}
		this.nodes = [];
		this.gains = [];
	}
}

function midiToHz(pitch: number): number {
	return 440 * Math.pow(2, (pitch - 69) / 12);
}

export const libraryMidiPreviewPlayer = new LibraryMidiPreviewPlayer();
