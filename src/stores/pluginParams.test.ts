import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

const { cachedPluginParamText, pluginParamText } = await import('./pluginParams');

describe('pluginParamText', () => {
	beforeEach(() => {
		invoke.mockReset();
	});

	it('asks the engine once per value and remembers the answer', async () => {
		invoke.mockResolvedValue('-6.0 dB');

		expect(await pluginParamText('fx1', 0, 0.5)).toBe('-6.0 dB');
		expect(await pluginParamText('fx1', 0, 0.5)).toBe('-6.0 dB');

		expect(invoke).toHaveBeenCalledTimes(1);
		expect(invoke).toHaveBeenCalledWith('get_plugin_param_text', {
			instanceKey: 'fx1',
			nativeId: 0,
			value: 0.5
		});
		expect(cachedPluginParamText('fx1', 0, 0.5)).toBe('-6.0 dB');
	});

	it('remembers that a plugin has no text for a value', async () => {
		invoke.mockResolvedValue(null);

		expect(await pluginParamText('fx1', 7, 10)).toBeNull();
		expect(await pluginParamText('fx1', 7, 10)).toBeNull();

		expect(invoke).toHaveBeenCalledTimes(1);
	});

	it('keeps values of different parameters apart', async () => {
		invoke.mockResolvedValueOnce('a').mockResolvedValueOnce('b');

		expect(await pluginParamText('fx2', 1, 1)).toBe('a');
		expect(await pluginParamText('fx2', 2, 1)).toBe('b');
		expect(cachedPluginParamText('fx2', 3, 1)).toBeUndefined();
	});

	it('shows no text when the engine call fails', async () => {
		invoke.mockRejectedValue(new Error('no engine'));

		expect(await pluginParamText('fx3', 0, 1)).toBeNull();
	});
});
