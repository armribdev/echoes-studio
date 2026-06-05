import { writable } from 'svelte/store';

export type StatusLevel = 'info' | 'success' | 'error';

export interface StatusMessage {
  text: string;
  level: StatusLevel;
}

export const statusMessage = writable<StatusMessage | null>(null);

export interface PianoRollStatusInfo {
	hoverLabel: string;
	noteMeta: string;
	clipMeta: string;
}

export const pianoRollStatusInfo = writable<PianoRollStatusInfo | null>(null);

let _clearTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * Display a message in the status bar, auto-clearing after `durationMs`.
 */
export function notify(text: string, level: StatusLevel = 'info', durationMs = 4000): void {
  if (_clearTimer !== null) clearTimeout(_clearTimer);
  statusMessage.set({ text, level });
  _clearTimer = setTimeout(() => {
    statusMessage.set(null);
    _clearTimer = null;
  }, durationMs);
}
