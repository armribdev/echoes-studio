import { writable } from 'svelte/store';

const STORAGE_KEY = 'echoes.metronome';

interface MetronomeSettings {
	enabled: boolean;
	countIn: boolean;
}

function loadSettings(): MetronomeSettings {
	try {
		const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}');
		return { enabled: stored.enabled === true, countIn: stored.countIn === true };
	} catch {
		return { enabled: false, countIn: false };
	}
}

function persist(settings: MetronomeSettings) {
	try {
		localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
	} catch {
		// Storage unavailable: the choice only lasts for this session.
	}
}

const initial = loadSettings();

/** Click while the transport plays (live monitoring only, never exported). */
export const metronomeEnabled = writable(initial.enabled);

/** One bar of click before a recording starts from a stopped transport. */
export const countInEnabled = writable(initial.countIn);

metronomeEnabled.subscribe((enabled) => persist({ ...loadSettings(), enabled }));
countInEnabled.subscribe((countIn) => persist({ ...loadSettings(), countIn }));
