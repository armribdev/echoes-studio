/**
 * Transport bridge — syncs the Svelte transport store with runtime engine events.
 *
 * All user-initiated transport actions go through the command dispatcher;
 * this module applies the resulting PLAY_STARTED / PLAY_STOPPED / POSITION_UPDATED
 * events back into the local store consumed by the UI and Web Audio playback.
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { get } from 'svelte/store';
import { transport } from '../../stores/transport';
import { trackLevels } from '../../stores/trackLevels';
import {
	ev,
	type EngineEvent,
	type PositionUpdatedPayload,
	type LevelsUpdatedPayload
} from '../../shared/protocol/events';

export function connectTransportBridge(): () => void {
	let unlisten: UnlistenFn | null = null;

	listen<EngineEvent>('engine-event', ({ payload }) => {
		switch (payload.type) {
			case ev.PLAY_STARTED:
				transport.update((t) => ({ ...t, isPlaying: true }));
				break;
			case ev.PLAY_STOPPED:
				transport.update((t) => ({ ...t, isPlaying: false }));
				break;
			case ev.POSITION_UPDATED: {
				const p = payload.payload as PositionUpdatedPayload;
				if (typeof p.position === 'number' && p.position >= 0 && !get(transport).isScrubbing) {
					transport.update((t) => ({ ...t, position: p.position }));
				}
				if (p.tracks) {
					trackLevels.set(p.tracks);
				}
				break;
			}
			case ev.LEVELS_UPDATED: {
				const p = payload.payload as LevelsUpdatedPayload;
				if (p.tracks) {
					trackLevels.set(p.tracks);
				}
				break;
			}
		}
	})
		.then((fn) => {
			unlisten = fn;
		})
		.catch(console.error);

	return () => {
		unlisten?.();
	};
}
