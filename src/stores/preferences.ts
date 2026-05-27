import { writable } from 'svelte/store';

export interface AudioPreferences {
	inputDevice: string;
	outputDevice: string;
	sampleRate: 44100 | 48000 | 88200 | 96000;
	bufferSize: 64 | 128 | 256 | 512 | 1024 | 2048;
	bitDepth: 16 | 24 | 32;
	inputChannels: number;
	outputChannels: number;
}

export interface ProjectPreferences {
	defaultFolder: string;
	autoSave: boolean;
	autoSaveIntervalSec: number;
	autoSaveMaxBackups: number;
	defaultBpm: number;
	defaultNumerator: number;
	defaultDenominator: number;
}

export interface InterfacePreferences {
	showMinimap: boolean;
}

export interface Preferences {
	audio: AudioPreferences;
	project: ProjectPreferences;
	interface: InterfacePreferences;
}

const STORAGE_KEY = 'echoes-studio-preferences';

function defaultPreferences(): Preferences {
	return {
		audio: {
			inputDevice: 'default',
			outputDevice: 'default',
			sampleRate: 48000,
			bufferSize: 256,
			bitDepth: 24,
			inputChannels: 2,
			outputChannels: 2
		},
		project: {
			defaultFolder: '',
			autoSave: true,
			autoSaveIntervalSec: 60,
			autoSaveMaxBackups: 5,
			defaultBpm: 120,
			defaultNumerator: 4,
			defaultDenominator: 4
		},
		interface: {
			showMinimap: false
		}
	};
}

function loadPreferences(): Preferences {
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		if (raw) {
			const parsed = JSON.parse(raw) as Partial<Preferences>;
			const defaults = defaultPreferences();
			return {
				audio: { ...defaults.audio, ...parsed.audio },
				project: { ...defaults.project, ...parsed.project },
				interface: { ...defaults.interface, ...parsed.interface }
			};
		}
	} catch {
		// Ignore
	}
	return defaultPreferences();
}

function createPreferencesStore() {
	const { subscribe, set, update } = writable<Preferences>(loadPreferences());

	function persist(prefs: Preferences) {
		try {
			localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
		} catch {
			// Ignore
		}
	}

	return {
		subscribe,
		set(prefs: Preferences) {
			persist(prefs);
			set(prefs);
		},
		update(fn: (p: Preferences) => Preferences) {
			update((p) => {
				const next = fn(p);
				persist(next);
				return next;
			});
		},
		reset() {
			const defaults = defaultPreferences();
			persist(defaults);
			set(defaults);
		}
	};
}

export const preferences = createPreferencesStore();
export const settingsPanelOpen = writable(false);
