import { get, writable } from 'svelte/store';
import {
	isAudioPath,
	assetIdFromPath,
	isMidiLibraryKind,
	type LibraryAssetKind
} from '../library/types';
import { probeAudioDuration, type ImportAudioOptions } from './importClip';

export type AudioFileDragSource = 'library' | 'os';

export interface AudioFileDragState {
	path: string;
	source: AudioFileDragSource;
	durationSec: number | null;
	assetId: string | null;
	/** Phase 3: MIDI/chord drag payload. */
	kind?: LibraryAssetKind;
	lengthBeats?: number | null;
}

export interface AudioFileDragPreview {
	trackIndex: number;
	startSec: number;
	durationSec: number | null;
}

/** Pointeur pendant un drag library (Tauri désactive le HTML5 DnD interne). */
export interface AudioFileDragPointer {
	clientX: number;
	clientY: number;
}

export type AudioFileDragDropEvent = {
	clientX: number;
	clientY: number;
};

/** Session active (library pointer-drag ou drop OS via Tauri). */
export const audioFileDrag = writable<AudioFileDragState | null>(null);

/** Position curseur pendant un drag library — MainPanel met à jour le ghost. */
export const audioFileDragPointer = writable<AudioFileDragPointer | null>(null);

/**
 * Demande de drop library (pointerup). MainPanel consomme et remet à null.
 * Évite le HTML5 DnD cassé par `dragDropEnabled` côté Tauri/WebView2.
 */
export const audioFileDragDrop = writable<AudioFileDragDropEvent | null>(null);

let probeGeneration = 0;

export function beginAudioFileDrag(opts: {
	path: string;
	source: AudioFileDragSource;
	durationSec?: number | null;
	assetId?: string | null;
	kind?: LibraryAssetKind;
	lengthBeats?: number | null;
}): AudioFileDragState | null {
	const kind = opts.kind ?? 'one_shot';
	const isMidi = isMidiLibraryKind(kind);
	if (!isMidi && !isAudioPath(opts.path)) return null;

	probeGeneration += 1;
	const generation = probeGeneration;
	const state: AudioFileDragState = {
		path: opts.path,
		source: opts.source,
		durationSec: opts.durationSec != null && opts.durationSec > 0 ? opts.durationSec : null,
		assetId: opts.assetId ?? assetIdFromPath(opts.path),
		kind,
		lengthBeats: opts.lengthBeats ?? null
	};
	audioFileDrag.set(state);

	if (!isMidi && state.durationSec == null) {
		void probeAudioDuration(opts.path)
			.then((dur) => {
				if (generation !== probeGeneration) return;
				const cur = get(audioFileDrag);
				if (!cur || cur.path !== opts.path) return;
				audioFileDrag.set({ ...cur, durationSec: dur });
			})
			.catch(() => {});
	}

	return state;
}

export function moveAudioFileDragPointer(clientX: number, clientY: number) {
	audioFileDragPointer.set({ clientX, clientY });
}

export function requestAudioFileDragDrop(clientX: number, clientY: number) {
	audioFileDragDrop.set({ clientX, clientY });
}

export function getActiveAudioFileDrag(): AudioFileDragState | null {
	return get(audioFileDrag);
}

export function endAudioFileDrag(): void {
	probeGeneration += 1;
	audioFileDrag.set(null);
	audioFileDragPointer.set(null);
	audioFileDragDrop.set(null);
}

export function isAudioFileDragActive(): boolean {
	return get(audioFileDrag) != null;
}

export function importOptionsFromAudioDrag(
	trackId: string,
	startSec: number,
	drag: AudioFileDragState
): ImportAudioOptions {
	const durationSec = drag.durationSec;
	return {
		trackId,
		filePath: drag.path,
		startSec,
		knownDurationSec: durationSec ?? undefined,
		endSec: durationSec != null ? startSec + durationSec : undefined
	};
}
