import type { LibraryAssetKind, LibraryOpenContext, LibrarySourceId, LaneFocus } from './types';
import { isAudioLibraryKind, isMidiLibraryKind } from './types';

export function laneFocusForTrack(track: {
	clips?: unknown[];
	midi_clips?: unknown[];
} | null | undefined): LaneFocus {
	if (!track) return 'neutral';
	const hasAudio = (track.clips?.length ?? 0) > 0;
	const hasMidi = (track.midi_clips?.length ?? 0) > 0;
	if (!hasAudio && !hasMidi) return 'neutral';
	if (hasAudio && !hasMidi) return 'audio';
	if (hasMidi && !hasAudio) return 'midi';
	return 'neutral';
}

export function openContextFromLaneFocus(focus: LaneFocus, hasTrack: boolean): LibraryOpenContext {
	if (!hasTrack) return 'track_empty';
	switch (focus) {
		case 'audio':
			return 'track_audio_lane';
		case 'midi':
			return 'track_midi_lane';
		case 'neutral':
		default:
			return 'track_empty';
	}
}

export function defaultSourceForContext(context: LibraryOpenContext): LibrarySourceId {
	switch (context) {
		case 'track_midi_lane':
			return 'chords';
		case 'track_audio_lane':
		case 'track_empty':
		case 'playhead_insert':
		default:
			return 'audio';
	}
}

export function canDropAssetOnLane(kind: LibraryAssetKind, lane: LaneFocus): boolean {
	if (lane === 'neutral') return true;
	if (lane === 'audio') return isAudioLibraryKind(kind);
	if (lane === 'midi') return isMidiLibraryKind(kind);
	return false;
}
