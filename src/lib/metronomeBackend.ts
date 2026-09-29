import { invoke } from '@tauri-apps/api/core';

/** Turn the engine's metronome click on or off (ADR-026). */
export async function setMetronomeEnabled(enabled: boolean): Promise<void> {
	await invoke<void>('set_metronome_enabled', { enabled });
}
