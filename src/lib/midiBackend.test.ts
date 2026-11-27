import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { applyMidiPreferences, type MidiInputDeviceInfo } from './midiBackend';

const invokeMock = vi.mocked(invoke);

function seeLiveDevices(devices: MidiInputDeviceInfo[]) {
	invokeMock.mockImplementation(async (command: string) =>
		command === 'list_midi_input_devices' ? devices : undefined
	);
}

function commandsSent(): string[] {
	return invokeMock.mock.calls
		.map(([command]) => command)
		.filter(
			(command) => !['list_midi_input_devices', 'set_midi_local_control_off'].includes(command)
		);
}

describe('applyMidiPreferences', () => {
	beforeEach(() => invokeMock.mockReset());

	it('switches MPE on a device that is already open without reopening it', async () => {
		seeLiveDevices([{ id: 'Seaboard', name: 'Seaboard', enabled: true, mpe: false }]);

		await applyMidiPreferences({
			localControlOff: false,
			devices: [{ name: 'Seaboard', alias: '', enabled: true, mpe: true }]
		});

		expect(commandsSent()).toEqual(['set_midi_input_mpe']);
		expect(invokeMock).toHaveBeenCalledWith('set_midi_input_mpe', {
			deviceId: 'Seaboard',
			mpe: true
		});
	});

	it('sets MPE before opening a device so its first notes are translated', async () => {
		seeLiveDevices([{ id: 'Seaboard', name: 'Seaboard', enabled: false, mpe: false }]);

		await applyMidiPreferences({
			localControlOff: false,
			devices: [{ name: 'Seaboard', alias: '', enabled: true, mpe: true }]
		});

		expect(commandsSent()).toEqual(['set_midi_input_mpe', 'set_midi_input_enabled']);
	});

	it('leaves a device alone when nothing changed', async () => {
		seeLiveDevices([{ id: 'Keys', name: 'Keys', enabled: true, mpe: false }]);

		await applyMidiPreferences({
			localControlOff: false,
			devices: [{ name: 'Keys', alias: '', enabled: true, mpe: false }]
		});

		expect(commandsSent()).toEqual([]);
	});
});
