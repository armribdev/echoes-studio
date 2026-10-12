import { invoke } from '@tauri-apps/api/core';
import type { LibraryAsset } from './types';

/** Shape returned by the `ensure_factory_drums` command. */
export interface FactoryDrumFile {
	id: string;
	name: string;
	note: number;
	path: string;
	duration_sec: number;
}

export function factoryDrumAsset(file: FactoryDrumFile): LibraryAsset {
	return {
		id: file.id,
		kind: 'one_shot',
		path: file.path,
		name: file.name,
		extension: 'wav',
		durationSec: file.duration_sec,
		origin: 'factory',
		tags: ['drums']
	};
}

export async function loadFactoryDrumAssets(): Promise<LibraryAsset[]> {
	const files = await invoke<FactoryDrumFile[]>('ensure_factory_drums');
	return files.map(factoryDrumAsset);
}

/** Sounds first (pad order), then the drum MIDI patterns already listed. */
export function drumIdsWithSoundsFirst(currentIds: string[], soundIds: string[]): string[] {
	const sounds = new Set(soundIds);
	return [...soundIds, ...currentIds.filter((id) => !sounds.has(id))];
}
