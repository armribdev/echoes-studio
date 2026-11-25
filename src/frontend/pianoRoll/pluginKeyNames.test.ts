import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
	fetchPluginNoteNames,
	keyNamesOf,
	sameKeyNames,
	type PluginNoteName
} from './pluginKeyNames';

const named = (key: number, name: string, port = -1, channel = -1): PluginNoteName => ({
	name,
	port,
	key,
	channel
});

describe('keyNamesOf', () => {
	it('names each key the plugin names', () => {
		const names = keyNamesOf([named(36, 'Kick'), named(38, 'Snare')]);
		expect(names.get(36)).toBe('Kick');
		expect(names.get(38)).toBe('Snare');
		expect(names.get(37)).toBeUndefined();
	});

	it('does not label a row with a name given to every key', () => {
		expect(keyNamesOf([named(-1, 'Anything')]).size).toBe(0);
	});

	it('keeps the first name of a key named for several ports or channels', () => {
		const names = keyNamesOf([named(36, 'Kick', 0, 9), named(36, 'Kick (layer)', 0, 10)]);
		expect(names.get(36)).toBe('Kick');
	});

	it('skips an empty name so the usual label stays', () => {
		expect(keyNamesOf([named(36, '')]).size).toBe(0);
	});

	it('names nothing for a plugin without names', () => {
		expect(keyNamesOf([]).size).toBe(0);
	});
});

describe('sameKeyNames', () => {
	it('is true for the same names however they were listed', () => {
		const a = keyNamesOf([named(36, 'Kick'), named(38, 'Snare')]);
		const b = keyNamesOf([named(38, 'Snare'), named(36, 'Kick')]);
		expect(sameKeyNames(a, b)).toBe(true);
	});

	it('is false when a name, a key or the count differs', () => {
		const kick = keyNamesOf([named(36, 'Kick')]);
		expect(sameKeyNames(kick, keyNamesOf([named(36, 'Bass drum')]))).toBe(false);
		expect(sameKeyNames(kick, keyNamesOf([named(37, 'Kick')]))).toBe(false);
		expect(sameKeyNames(kick, keyNamesOf([named(36, 'Kick'), named(38, 'Snare')]))).toBe(false);
		expect(sameKeyNames(kick, keyNamesOf([]))).toBe(false);
	});
});

describe('fetchPluginNoteNames', () => {
	beforeEach(() => {
		vi.mocked(invoke).mockReset();
	});

	it('asks the placement and returns what the plugin names', async () => {
		vi.mocked(invoke).mockResolvedValue([named(36, 'Kick')]);
		expect(await fetchPluginNoteNames('instrument:t1')).toEqual([named(36, 'Kick')]);
		expect(invoke).toHaveBeenCalledWith('get_plugin_note_names', {
			instanceKey: 'instrument:t1'
		});
	});

	it('gives no names for an unknown or busy placement', async () => {
		vi.mocked(invoke).mockResolvedValue(null);
		expect(await fetchPluginNoteNames('instrument:t1')).toEqual([]);
	});

	it('gives no names when the command fails', async () => {
		vi.mocked(invoke).mockRejectedValue(new Error('no plugin'));
		expect(await fetchPluginNoteNames('instrument:t1')).toEqual([]);
	});
});
