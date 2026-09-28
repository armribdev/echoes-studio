import { convertFileSrc } from '@tauri-apps/api/core';

/**
 * Isolated UI preview player (ADR-023 Phase 1).
 * Does not touch arrange playhead, clips, or the engine audio callback.
 */
class LibraryPreviewPlayer {
	private ctx: AudioContext | null = null;
	private source: AudioBufferSourceNode | null = null;
	private playingPath: string | null = null;
	private generation = 0;

	private ensureContext(): AudioContext {
		if (!this.ctx) {
			this.ctx = new AudioContext();
		}
		return this.ctx;
	}

	isPlaying(path?: string): boolean {
		if (!this.playingPath) return false;
		return path == null ? true : this.playingPath === path;
	}

	async play(path: string): Promise<void> {
		this.stop();
		const gen = ++this.generation;
		const ctx = this.ensureContext();
		if (ctx.state === 'suspended') {
			await ctx.resume();
		}

		const url = convertFileSrc(path);
		const response = await fetch(url);
		if (!response.ok) {
			throw new Error(`Preview fetch failed (${response.status})`);
		}
		const arrayBuffer = await response.arrayBuffer();
		if (gen !== this.generation) return;

		const buffer = await ctx.decodeAudioData(arrayBuffer.slice(0));
		if (gen !== this.generation) return;

		const source = ctx.createBufferSource();
		source.buffer = buffer;
		source.connect(ctx.destination);
		source.onended = () => {
			if (this.source === source) {
				this.source = null;
				this.playingPath = null;
			}
		};
		this.source = source;
		this.playingPath = path;
		source.start(0);
	}

	async toggle(path: string): Promise<void> {
		if (this.isPlaying(path)) {
			this.stop();
			return;
		}
		await this.play(path);
	}

	stop(): void {
		this.generation += 1;
		if (this.source) {
			try {
				this.source.stop();
			} catch {
				/* already stopped */
			}
			try {
				this.source.disconnect();
			} catch {
				/* ignore */
			}
			this.source = null;
		}
		this.playingPath = null;
	}
}

export const libraryPreviewPlayer = new LibraryPreviewPlayer();
