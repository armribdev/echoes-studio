import { writable } from 'svelte/store';
import type { MidiInputDeviceInfo } from '../lib/midiBackend';

/**
 * Hardware MIDI input devices currently visible to the OS (ADR-022 amendment:
 * live hotplug detection). Fed by the `midi-devices-changed` backend event
 * subscribed once in `routes/+page.svelte` for the app's lifetime — components
 * just read this store instead of polling `listMidiInputDevices()` themselves.
 */
export const midiDeviceList = writable<MidiInputDeviceInfo[]>([]);
