import { get } from 'svelte/store';
import { createTrack, addMidiClip } from '../commands/dispatcher';
import { importAudioToTrack } from '../audio/importClip';
import { projectEventStore } from '../../shared/project/eventStore';
import { selectedTrack } from '../../stores/selectedTrack';
import { transport } from '../../stores/transport';
import { tempo } from '../../stores/tempo';
import { notify } from '../../stores/status';
import { ev, isEngineEvent, type TrackCreatedPayload } from '../../shared/protocol/events';
import type { CommandResult } from '../../shared/protocol/commands';
import { libraryStore } from './libraryStore';
import { canDropAssetOnLane, laneFocusForTrack } from './laneFocus';
import { beatsToSeconds, resolveLibraryMidiNotes } from './resolveMidi';
import { isMidiLibraryKind, type LibraryAsset } from './types';

function trackIdFromCreateResult(result: CommandResult): string | null {
	for (const raw of result.events) {
		if (!isEngineEvent(raw) || raw.type !== ev.TRACK_CREATED) continue;
		return (raw.payload as TrackCreatedPayload).id;
	}
	return null;
}

async function ensureTrackId(preferredTrackId: string | null | undefined): Promise<string | null> {
	if (preferredTrackId) return preferredTrackId;
	const selected = get(selectedTrack).trackId;
	if (selected) return selected;
	const first = get(projectEventStore).tracks[0]?.id;
	if (first) return first;

	try {
		const trackCount = get(projectEventStore).tracks.length;
		const result = await createTrack(`Piste ${trackCount + 1}`);
		return (
			trackIdFromCreateResult(result) ??
			get(projectEventStore).tracks[get(projectEventStore).tracks.length - 1]?.id ??
			null
		);
	} catch (e) {
		notify(`Erreur création piste : ${(e as Error)?.message ?? String(e)}`, 'error');
		return null;
	}
}

function assertCompatibleLane(asset: LibraryAsset, trackId: string): boolean {
	const track = get(projectEventStore).tracks.find((t) => t.id === trackId);
	if (canDropAssetOnLane(asset.kind, laneFocusForTrack(track))) return true;
	notify(
		isMidiLibraryKind(asset.kind)
			? 'Drop incompatible (piste audio)'
			: 'Drop incompatible (piste MIDI)',
		'error',
	);
	return false;
}

/** Insert a library asset at the playhead (creates a track if the project is empty). */
export async function insertLibraryAsset(opts: {
	asset: LibraryAsset;
	trackId?: string | null;
	startSec?: number;
}): Promise<boolean> {
	if (isMidiLibraryKind(opts.asset.kind)) {
		return insertMidiLibraryAsset(opts);
	}
	return insertAudioLibraryAsset(opts);
}

async function insertAudioLibraryAsset(opts: {
	asset: LibraryAsset;
	trackId?: string | null;
	startSec?: number;
}): Promise<boolean> {
	const trackId = await ensureTrackId(opts.trackId);
	if (!trackId) return false;
	if (!assertCompatibleLane(opts.asset, trackId)) return false;

	try {
		libraryStore.stopPreview();
		await importAudioToTrack({
			trackId,
			filePath: opts.asset.path,
			startSec: opts.startSec ?? get(transport).position,
			knownDurationSec: opts.asset.durationSec,
		});
		libraryStore.recordRecent(opts.asset.id);
		return true;
	} catch {
		/* importAudioToTrack already notified */
		return false;
	}
}

export async function insertMidiLibraryAsset(opts: {
	asset: LibraryAsset;
	trackId?: string | null;
	startSec?: number;
}): Promise<boolean> {
	const trackId = await ensureTrackId(opts.trackId);
	if (!trackId) return false;
	if (!assertCompatibleLane(opts.asset, trackId)) return false;

	const notes = resolveLibraryMidiNotes(opts.asset);
	if (notes.length === 0) {
		notify('Motif MIDI vide', 'error');
		return false;
	}

	const bpm = get(tempo).bpm;
	const lengthBeats = opts.asset.lengthBeats ?? Math.max(...notes.map((n) => n.start + n.duration), 4);
	const startSec = opts.startSec ?? get(transport).position;
	const endSec = startSec + beatsToSeconds(lengthBeats, bpm);

	try {
		libraryStore.stopPreview();
		await addMidiClip(trackId, startSec, endSec, {
			notes: notes.map((n, i) => ({
				id: `lib-${i}`,
				pitch: n.pitch,
				start: n.start,
				duration: n.duration,
				velocity: n.velocity,
			})),
		});
		libraryStore.recordRecent(opts.asset.id);
		return true;
	} catch (e) {
		notify(`Erreur insert MIDI : ${(e as Error)?.message ?? String(e)}`, 'error');
		return false;
	}
}
