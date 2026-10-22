import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

const {
	activePresetKey,
	fetchPluginPresets,
	groupPresets,
	loadPluginPreset,
	presetErrorMessage,
	presetKey,
	samePresets
} = await import('./pluginPresets');
import type { PluginPreset } from './pluginPresets';

const FACTORY = 1;
const USER = 2;

function preset(name: string, flags = FACTORY, loadKey: string | null = name): PluginPreset {
	return {
		name,
		load_key: loadKey,
		location_kind: 1,
		location: null,
		provider_id: 'provider',
		flags,
		creator: null,
		description: null,
		features: [],
		soundpack: null,
		active: false
	};
}

describe('groupPresets', () => {
	it('puts factory presets first, then the user ones, then the rest', () => {
		const groups = groupPresets([
			preset('Mine', USER),
			preset('Demo', 4),
			preset('Pad', FACTORY),
			preset('Both', FACTORY | USER)
		]);

		expect(groups.map((entry) => entry.group)).toEqual(['factory', 'user', 'other']);
		expect(groups[0].presets.map((p) => p.name)).toEqual(['Pad', 'Both']);
		expect(groups[1].presets.map((p) => p.name)).toEqual(['Mine']);
		expect(groups[2].presets.map((p) => p.name)).toEqual(['Demo']);
	});

	it('keeps the order the engine gave inside a group and leaves out empty groups', () => {
		const groups = groupPresets([preset('B', USER), preset('A', USER)]);

		expect(groups).toHaveLength(1);
		expect(groups[0].presets.map((p) => p.name)).toEqual(['B', 'A']);
		expect(groupPresets([])).toEqual([]);
	});
});

describe('samePresets', () => {
	it('is true for the same list and false once a preset is added, renamed or moved', () => {
		const list = [preset('A'), preset('B')];

		expect(samePresets(list, [preset('A'), preset('B')])).toBe(true);
		expect(samePresets(list, [preset('A')])).toBe(false);
		expect(samePresets(list, [preset('A'), { ...preset('B'), name: 'B2' }])).toBe(false);
		expect(samePresets(list, [preset('A'), preset('B', FACTORY, 'other-key')])).toBe(false);
	});

	it('is false once the plugin names another preset as the one in use', () => {
		const list = [preset('A'), preset('B')];

		expect(samePresets(list, [{ ...preset('A'), active: true }, preset('B')])).toBe(false);
		expect(
			samePresets([{ ...preset('A'), active: true }], [{ ...preset('A'), active: true }])
		).toBe(true);
	});

	it('keys a preset by where it lives and its key, as the engine checks it', () => {
		const file = {
			...preset('Warm'),
			location_kind: 0,
			location: 'C:\\P\\warm.fxp',
			load_key: null
		};

		expect(JSON.parse(presetKey(file))).toEqual([0, 'C:\\P\\warm.fxp', null]);
		expect(presetKey(preset('A'))).not.toBe(presetKey(preset('B')));
	});
});

describe('activePresetKey', () => {
	it('is the key of the preset the plugin says is in use', () => {
		const list = [preset('A'), { ...preset('B'), active: true }, preset('C')];

		expect(activePresetKey(list)).toBe(presetKey(list[1]));
	});

	it('is empty when the plugin cannot tell, as for a CLAP plugin', () => {
		expect(activePresetKey([preset('A'), preset('B')])).toBe('');
		expect(activePresetKey([])).toBe('');
	});
});

describe('the engine commands', () => {
	beforeEach(() => {
		invoke.mockReset();
	});

	it('lists the presets of a placement', async () => {
		invoke.mockResolvedValue([preset('A')]);

		expect(await fetchPluginPresets('instrument:t1')).toEqual([preset('A')]);
		expect(invoke).toHaveBeenCalledWith('list_plugin_presets', { instanceKey: 'instrument:t1' });
	});

	it('lists none when the engine cannot answer', async () => {
		invoke.mockRejectedValue('boom');

		expect(await fetchPluginPresets('fx1')).toEqual([]);
	});

	it('loads a preset by its location and key', async () => {
		invoke.mockResolvedValue(undefined);
		const file = {
			...preset('Warm'),
			location_kind: 0,
			location: 'C:\\P\\warm.fxp',
			load_key: null
		};

		await loadPluginPreset('fx1', file);

		expect(invoke).toHaveBeenCalledWith('load_plugin_preset', {
			instanceKey: 'fx1',
			locationKind: 0,
			location: 'C:\\P\\warm.fxp',
			loadKey: null
		});
	});

	it('passes the plugin refusal on', async () => {
		invoke.mockRejectedValue('unreadable preset');

		await expect(loadPluginPreset('fx1', preset('A'))).rejects.toBe('unreadable preset');
	});
});

describe('presetErrorMessage', () => {
	it('reads a string, an error, or anything else', () => {
		expect(presetErrorMessage('unreadable preset')).toBe('unreadable preset');
		expect(presetErrorMessage(new Error('busy'))).toBe('busy');
		expect(presetErrorMessage(42)).toBe('42');
	});
});
