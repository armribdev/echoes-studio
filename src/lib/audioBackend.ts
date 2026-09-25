/**
 * audioBackend.ts — thin wrappers around the Tauri IPC commands that expose
 * audio device enumeration and settings application.
 *
 * These functions talk directly to the Rust `audio_settings` commands; they
 * are the only place in the frontend that should `invoke` those commands.
 */

import type { PluginIsolation } from '../stores/preferences';
import { invoke } from '@tauri-apps/api/core';

// ── Types (mirror of src-tauri/src/audio_settings.rs) ────────────────────────

/** One audio device as returned by the backend. */
export interface AudioDeviceInfo {
	/** Machine-readable ID — pass back as `outputDevice` / `inputDevice`. */
	id: string;
	/** Human-readable display name for the device picker. */
	name: string;
}

/** Full device list returned by `get_audio_devices`. */
export interface AudioDeviceList {
	inputs: AudioDeviceInfo[];
	outputs: AudioDeviceInfo[];
}

/**
 * Audio settings sent to `apply_audio_settings`.
 *
 * Field names match the Rust `AudioSettings` struct (camelCase via serde).
 */
export interface AudioSettingsPayload {
	/** CPAL device name, or `"default"` for the system default. */
	outputDevice: string;
	/** CPAL device name, or `"default"` (reserved — not yet used). */
	inputDevice: string;
	/** Sample rate in Hz — must be one the device supports. */
	sampleRate: number;
	/** Buffer size in frames (e.g. 64, 128, 256, 512). */
	bufferSize: number;
	/**
	 * Number of output channels to activate.
	 * 1 = mono downmix (broadcast to all device channels so both ears hear it).
	 * 2+ = stereo / multi (L→even channels, R→odd channels).
	 * 0 = use all device channels (default when unset).
	 */
	outputChannels: number;
	/** When true, master bus applies tanh soft-clip before device output. */
	masterSoftClip: boolean;
}

// ── API ───────────────────────────────────────────────────────────────────────

/**
 * Fetch the list of audio input/output devices from the CPAL host.
 *
 * Always resolves — returns empty arrays on error so the UI can still render
 * the Settings panel with a sensible fallback.
 */
export async function listAudioDevices(): Promise<AudioDeviceList> {
	try {
		return await invoke<AudioDeviceList>('get_audio_devices');
	} catch (err) {
		console.error('[audioBackend] get_audio_devices failed:', err);
		const fallback: AudioDeviceInfo = { id: 'default', name: 'Par défaut (système)' };
		return { inputs: [fallback], outputs: [fallback] };
	}
}

/**
 * Return which of the standard candidate sample rates (44100, 48000, 88200,
 * 96000) are supported by the given output device.
 *
 * Always resolves — returns all candidates on error so every pill stays enabled.
 *
 * @param deviceId  CPAL device name, or `"default"`.
 */
export async function getSupportedSampleRates(deviceId: string): Promise<number[]> {
	try {
		return await invoke<number[]>('get_device_sample_rates', { deviceId });
	} catch (err) {
		console.error('[audioBackend] get_device_sample_rates failed:', err);
		return [44100, 48000, 88200, 96000];
	}
}

/**
 * Return the native channel count of a device.
 *
 * Pass `isInput = true` for input devices, `false` for output devices.
 * Always resolves — returns 2 (stereo) on error so the UI stays sensible.
 *
 * @param deviceId  CPAL device name, or `"default"`.
 * @param isInput   `true` for input, `false` for output.
 */
export async function getDeviceChannels(deviceId: string, isInput: boolean): Promise<number> {
	try {
		return await invoke<number>('get_device_channels', { deviceId, isInput });
	} catch (err) {
		console.error('[audioBackend] get_device_channels failed:', err);
		return 2;
	}
}

/**
 * Apply new audio I/O settings to the running CPAL engine.
 *
 * This restarts the CPAL stream (brief audio interruption of ~50 ms) and
 * recompiles the audio graph with the new sample rate.
 *
 * @throws `string` error message if the device cannot be opened.
 */
export async function applyAudioSettings(settings: AudioSettingsPayload): Promise<void> {
	await invoke<void>('apply_audio_settings', { settings });
}

/**
 * Which plugins run in their own process: the mode, plus the plugin ids isolated
 * everywhere. A change reloads the affected plugins.
 */
export async function setPluginIsolation(mode: PluginIsolation, alwaysIsolated: string[]): Promise<void> {
	await invoke<void>('set_plugin_isolation', { mode, alwaysIsolated });
}

/**
 * Update the active output channel count **without** restarting the stream.
 *
 * Takes effect within one audio callback block (~5 ms). Used for live preview
 * while the user clicks +/− in the Settings panel, and for instant revert on
 * Cancel.
 *
 * `count = 0` restores the "use all hardware channels" default.
 */
export async function setActiveOutputChannels(count: number): Promise<void> {
	try {
		await invoke<void>('set_active_output_channels', { count });
	} catch (err) {
		console.error('[audioBackend] set_active_output_channels failed:', err);
	}
}
