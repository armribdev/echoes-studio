/**
 * midiBackend.ts — thin wrappers around the Tauri IPC commands that expose
 * hardware MIDI input device enumeration and track arming (ADR-022).
 *
 * These functions talk directly to the Rust MIDI commands; they are the only
 * place in the frontend that should `invoke` those commands.
 */

import { invoke } from '@tauri-apps/api/core';
import type { MidiPreferences } from '../stores/preferences';
import type { MidiControlPoint } from '../types/global';

// ── Types (mirror of src-tauri/src/midi_input.rs) ────────────────────────────

/** One hardware MIDI input device as returned by the backend. */
export interface MidiInputDeviceInfo {
	/** Device name — used both as display name and as the id passed back. */
	id: string;
	name: string;
	/** Whether this device currently has an open connection. */
	enabled: boolean;
}

/**
 * Backend event fired when a device connects or disconnects (live hotplug
 * detection, ADR-022 amendment). Payload: `MidiInputDeviceInfo[]`.
 */
export const MIDI_DEVICES_EVENT = 'midi-devices-changed';

// ── API ───────────────────────────────────────────────────────────────────────

/**
 * Fetch the list of visible hardware MIDI input devices.
 *
 * Always resolves — returns an empty array on error so the Settings panel
 * still renders with a sensible fallback.
 */
export async function listMidiInputDevices(): Promise<MidiInputDeviceInfo[]> {
	try {
		return await invoke<MidiInputDeviceInfo[]>('list_midi_input_devices');
	} catch (err) {
		console.error('[midiBackend] list_midi_input_devices failed:', err);
		return [];
	}
}

/**
 * Enable or disable hardware MIDI input for a device.
 *
 * @throws `string` error message if the device cannot be opened.
 */
export async function setMidiInputEnabled(deviceId: string, enabled: boolean): Promise<void> {
	await invoke<void>('set_midi_input_enabled', { deviceId, enabled });
}

/** Silence (or give back) the enabled keyboards' own sound (MIDI Local Control). */
export async function setMidiLocalControlOff(off: boolean): Promise<void> {
	await invoke<void>('set_midi_local_control_off', { off });
}

/**
 * Bring the engine in line with the MIDI preferences: Local Control first, so
 * devices opened next are silenced too, then open/close each visible device.
 * Devices remembered but not plugged in are left alone. Returns the names of
 * devices that could not be opened or closed.
 */
export async function applyMidiPreferences(midi: MidiPreferences): Promise<string[]> {
	await setMidiLocalControlOff(midi.localControlOff);
	const failed: string[] = [];
	for (const device of await listMidiInputDevices()) {
		const known = midi.devices.find((d) => d.name === device.id);
		const wanted = known?.enabled ?? false;
		if (wanted === device.enabled) continue;
		try {
			await setMidiInputEnabled(device.id, wanted);
		} catch (err) {
			console.error('[midiBackend] set_midi_input_enabled failed:', device.id, err);
			failed.push(known?.alias || device.name);
		}
	}
	return failed;
}

/** Backend event fired when a MIDI recording ends. Payload: `RecordedMidiClip | null`. */
export const MIDI_RECORDING_FINISHED_EVENT = 'midi-recording-finished';

/** Mirror of `midi_recording::RecordedMidiClip`; `null` payload = no note played. */
export interface RecordedMidiClip {
	trackId: string;
	/** Seconds, same coordinate space as a MIDI clip. */
	start: number;
	end: number;
	notes: { id: string; pitch: number; start: number; duration: number; velocity: number }[];
	channelControls: MidiControlPoint[];
}

/** Backend event fired for every note edge while recording. Payload: `RecordedNote`. */
export const MIDI_RECORDING_NOTE_EVENT = 'midi-recording-note';

/** Mirror of `midi_recording::RecordedNote`. */
export interface RecordedNote {
	noteOn: boolean;
	pitch: number;
	velocity: number;
	/** Seconds on the timeline. */
	position: number;
}

/**
 * Returns the timeline position (seconds) where the recording starts. From a
 * stopped transport, `countInBars` bars of click play before it moves.
 * @throws `string` error message when no track is armed or a recording is running.
 */
export async function startMidiRecording(countInBars: number): Promise<number> {
	return invoke<number>('start_midi_recording', { countInBars });
}

/** The clip arrives through `MIDI_RECORDING_FINISHED_EVENT`. */
export async function stopMidiRecording(): Promise<void> {
	await invoke<void>('stop_midi_recording');
}

/**
 * Arm a track for hardware MIDI input, or disarm when `trackId` is `null`.
 * Arming only routes live notes to the track; recording is separate.
 */
export async function armTrackForMidiInput(trackId: string | null): Promise<void> {
	try {
		await invoke<void>('arm_track_for_midi_input', { trackId });
	} catch (err) {
		console.error('[midiBackend] arm_track_for_midi_input failed:', err);
	}
}
