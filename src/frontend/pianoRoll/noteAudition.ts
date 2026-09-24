import { invoke } from '@tauri-apps/api/core';

let held: { trackId: string; pitch: number } | null = null;
let releaseTimer: ReturnType<typeof setTimeout> | null = null;

function send(trackId: string, pitch: number, noteOn: boolean, velocity: number) {
	void invoke('preview_midi_note', { trackId, pitch, velocity, noteOn }).catch(() => {});
}

/** Joue la hauteur tant que le geste (pose ou déplacement) est en cours. */
export function auditionPitch(trackId: string, pitch: number, velocity: number) {
	if (releaseTimer) {
		clearTimeout(releaseTimer);
		releaseTimer = null;
	}
	const next = Math.max(0, Math.min(127, pitch | 0));
	const vel = Math.max(1, Math.min(127, velocity | 0));
	if (held && held.trackId === trackId && held.pitch === next) return;
	if (held) send(held.trackId, held.pitch, false, 0);
	held = { trackId, pitch: next };
	send(trackId, next, true, vel);
}

/** Relâche la note après l'attaque, pour qu'un clic bref reste audible. */
export function releaseAudition() {
	if (!held) return;
	const current = held;
	if (releaseTimer) clearTimeout(releaseTimer);
	releaseTimer = setTimeout(() => {
		releaseTimer = null;
		if (!held || held.trackId !== current.trackId || held.pitch !== current.pitch) return;
		held = null;
		send(current.trackId, current.pitch, false, 0);
	}, 140);
}
