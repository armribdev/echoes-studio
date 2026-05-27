/**
 * Transport bridge — syncs the Svelte transport store with runtime engine events.
 *
 * All user-initiated transport actions go through the command dispatcher;
 * this module applies the resulting PLAY_STARTED / PLAY_STOPPED / POSITION_UPDATED
 * events back into the local store consumed by the UI and Web Audio playback.
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { transport } from '../../stores/transport';
import { ev, type EngineEvent, type PositionUpdatedPayload } from '../../shared/protocol/events';

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
        const { position } = payload.payload as PositionUpdatedPayload;
        if (typeof position === 'number' && position >= 0) {
          transport.update((t) => ({ ...t, position }));
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
