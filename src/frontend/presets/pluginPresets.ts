import { invoke } from '@tauri-apps/api/core';

/** A preset a CLAP plugin offers (`list_plugin_presets`), as the engine reports it. */
export interface PluginPreset {
	name: string;
	load_key: string | null;
	location_kind: number;
	location: string | null;
	provider_id: string;
	flags: number;
	creator: string | null;
	description: string | null;
	features: string[];
	soundpack: string | null;
	/** The plugin says this preset is in use (a VST3 program); false when it cannot tell. */
	active: boolean;
}

export type PresetGroup = 'factory' | 'user' | 'other';

export interface PresetGroupEntries {
	group: PresetGroup;
	presets: PluginPreset[];
}

const IS_FACTORY_CONTENT = 1 << 0;
const IS_USER_CONTENT = 1 << 1;

const GROUP_ORDER: PresetGroup[] = ['factory', 'user', 'other'];

export const PRESET_GROUP_LABELS: Record<PresetGroup, string> = {
	factory: 'Usine',
	user: 'Utilisateur',
	other: 'Autres'
};

function presetGroup(preset: PluginPreset): PresetGroup {
	if (preset.flags & IS_FACTORY_CONTENT) return 'factory';
	if (preset.flags & IS_USER_CONTENT) return 'user';
	return 'other';
}

/** The non-empty groups in display order; presets keep the order the engine gave. */
export function groupPresets(presets: PluginPreset[]): PresetGroupEntries[] {
	return GROUP_ORDER.map((group) => ({
		group,
		presets: presets.filter((preset) => presetGroup(preset) === group)
	})).filter((entry) => entry.presets.length > 0);
}

/** Identifies a preset to the engine: where it lives and the plugin's key inside it. */
export function presetKey(preset: PluginPreset): string {
	return JSON.stringify([preset.location_kind, preset.location, preset.load_key]);
}

export function samePresets(a: PluginPreset[], b: PluginPreset[]): boolean {
	return (
		a.length === b.length &&
		a.every((preset, index) => {
			const other = b[index];
			return (
				presetKey(preset) === presetKey(other) &&
				preset.name === other.name &&
				preset.active === other.active
			);
		})
	);
}

/** The key of the preset the plugin says is in use, or '' when it cannot tell. */
export function activePresetKey(presets: PluginPreset[]): string {
	const active = presets.find((preset) => preset.active);
	return active ? presetKey(active) : '';
}

/** The presets of a loaded plugin placement; empty when it has none or is not loaded. */
export async function fetchPluginPresets(instanceKey: string): Promise<PluginPreset[]> {
	try {
		return await invoke<PluginPreset[]>('list_plugin_presets', { instanceKey });
	} catch {
		return [];
	}
}

/** Loads one of the listed presets; rejects with the plugin's own message when it refuses. */
export async function loadPluginPreset(instanceKey: string, preset: PluginPreset): Promise<void> {
	await invoke('load_plugin_preset', {
		instanceKey,
		locationKind: preset.location_kind,
		location: preset.location,
		loadKey: preset.load_key
	});
}

export function presetErrorMessage(error: unknown): string {
	if (typeof error === 'string') return error;
	return (error as Error)?.message ?? String(error);
}
