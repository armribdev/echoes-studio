import type { Clip } from '../../types/global';
import type { PendingImportClip, PeakPair } from './pendingImports';

/** Keeps Svelte `{#each}` identity across pending → engine handoff (anti-flash). */
const uiKeyByClipId = new Map<string, string>();

export function rememberClipUiKey(clipId: string, uiKey: string) {
	uiKeyByClipId.set(clipId, uiKey);
}

export function forgetClipUiKey(clipId: string) {
	uiKeyByClipId.delete(clipId);
}

export function clipUiKey(clipId: string): string {
	return uiKeyByClipId.get(clipId) ?? clipId;
}

export interface ClipDisplayItem {
	key: string;
	id: string;
	start: number;
	end: number;
	offset: number;
	sample_rate: number;
	peaks_64: PeakPair[];
	peaks_512: PeakPair[];
	peaks_4096: PeakPair[];
	pending: boolean;
	loadProgress: number;
	/** Set while this row is owned by an in-flight import slot. */
	pendingImportId?: string;
}

function normPath(p: string): string {
	return p.replace(/\\/g, '/').toLowerCase();
}

/**
 * Merge engine clips + pending imports for one track.
 * Settled slots keep streamed peaks. Unattached pendings also hide the engine
 * clip that just arrived from ADD_CLIP (same file/start) — otherwise a fresh
 * empty Clip mounts on top and the waveform flashes away during addClip.
 */
export function buildClipDisplayItems(
	clips: Clip[],
	pendings: PendingImportClip[],
	trackId: string,
): ClipDisplayItem[] {
	const trackPendings = pendings.filter((p) => {
		if (p.trackId === trackId) return true;
		if (p.settled && p.engineClipId && clips.some((c) => c.id === p.engineClipId)) {
			return true;
		}
		return false;
	});

	const maskedEngineIds = new Set(
		trackPendings.map((p) => p.engineClipId).filter((id): id is string => !!id),
	);

	const items: ClipDisplayItem[] = [];

	for (const p of trackPendings) {
		const engine = p.engineClipId ? clips.find((c) => c.id === p.engineClipId) : undefined;
		if (p.settled && p.engineClipId && !engine) continue;

		items.push({
			key: p.id,
			id: engine?.id ?? p.id,
			start: p.start,
			end: p.end,
			offset: 0,
			sample_rate: p.sampleRate || engine?.sample_rate || 48_000,
			peaks_64: p.peaks_64,
			peaks_512: p.peaks_512,
			peaks_4096: p.peaks_4096,
			pending: !p.engineClipId,
			loadProgress: p.engineClipId ? 1 : p.progress,
			pendingImportId: p.id,
		});
	}

	for (const c of clips) {
		if (maskedEngineIds.has(c.id)) continue;
		// ADD_CLIP lands in the store before attachEngineClip — hide that twin.
		const hiddenByInflightPending = trackPendings.some(
			(p) =>
				!p.engineClipId &&
				normPath(p.filePath) === normPath(c.file_path ?? '') &&
				Math.abs(p.start - c.start) < 0.1,
		);
		if (hiddenByInflightPending) continue;

		items.push({
			key: clipUiKey(c.id),
			id: c.id,
			start: c.start,
			end: c.end,
			offset: c.offset ?? 0,
			sample_rate: c.sample_rate,
			peaks_64: c.peaks_64 ?? [],
			peaks_512: c.peaks_512 ?? [],
			peaks_4096: c.peaks_4096 ?? [],
			pending: false,
			loadProgress: 1,
		});
	}

	return items;
}
