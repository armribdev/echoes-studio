<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { listen } from '@tauri-apps/api/event';
	import { invoke } from '@tauri-apps/api/core';
	import MenuBar from '../components/MenuBar.svelte';
	import MainPanel from '../components/MainPanel.svelte';
	import StatusBar from '../components/StatusBar.svelte';
	import LeftPanel from '../components/LeftPanel.svelte';
	import RightPanel from '../components/RightPanel.svelte';
	import BottomPanel from '../components/BottomPanel.svelte';
	// Subscribing to engineProject here triggers connect() on the event store,
	// which starts listening to "engine-event" and "project-loaded" channels.
	// The store then stays alive for the entire app lifetime.
	import { engineProject } from '../frontend/store/engineStore';
	import { registerShortcuts, syncMenuAccelerators } from '../frontend/shortcuts/keymap';
	import { isEditingText } from '../frontend/shortcuts/context';
	import { connectTransportBridge } from '../frontend/transport/bridge';
	import { connectLoopSync } from '../frontend/transport/loopSync';
	import SettingsPanel from '../components/SettingsPanel.svelte';
	import ProjectSettingsPanel from '../components/ProjectSettingsPanel.svelte';
	import MissingPluginsDialog from '../components/MissingPluginsDialog.svelte';
	import CrashSuspectDialog from '../components/CrashSuspectDialog.svelte';
	import GraphView from '../components/GraphView.svelte';
	import { settingsPanelOpen, preferences } from '../stores/preferences';
	import { setAppLocale, getAppLocale } from '../lib/i18n';
	import { setPluginIsolation } from '../lib/audioBackend';
	import {
		applyMidiPreferences,
		listMidiInputDevices,
		MIDI_DEVICES_EVENT
	} from '../lib/midiBackend';
	import type { MidiInputDeviceInfo } from '../lib/midiBackend';
	import { midiDeviceList } from '../stores/midiDevices';
	import { connectMidiRecording } from '../frontend/midiRecording';
	import { connectAudioRecording } from '../frontend/audioRecording';
	import { setInputMonitorEnabled } from '../lib/audioRecordingBackend';
	import { inputMonitorEnabled } from '../stores/audioRecording';
	import { setMetronomeEnabled } from '../lib/metronomeBackend';
	import { metronomeEnabled } from '../stores/metronome';
	import { projectSettingsPanelOpen } from '../stores/project';
	import { exportMixdown } from '../lib/exportMixdown';
	import { notify } from '../stores/status';
	import { leftPanel } from '../stores/leftPanel';
	import { viewMode } from '../stores/viewMode';
	import { graphSnapshot } from '../stores/graphSnapshot';
	import { selectedTrack } from '../stores/selectedTrack';

	// Register keyboard shortcuts and transport bridge once for the app lifetime.
	onMount(() => {
		document.documentElement.lang = getAppLocale();
		const unsubLocale = preferences.subscribe(({ interface: iface }) => {
			setAppLocale(iface.locale);
			document.documentElement.lang = iface.locale;
		});
		let appliedShortcuts = '';
		const unsubShortcuts = preferences.subscribe(({ shortcuts }) => {
			const key = JSON.stringify(shortcuts);
			if (key === appliedShortcuts) return;
			appliedShortcuts = key;
			syncMenuAccelerators(shortcuts);
		});
		let appliedIsolation = '';
		const unsubIsolation = preferences.subscribe(({ audio }) => {
			const key = JSON.stringify([audio.pluginIsolation, audio.alwaysIsolated]);
			if (key === appliedIsolation) return;
			appliedIsolation = key;
			setPluginIsolation(audio.pluginIsolation, audio.alwaysIsolated).catch((e) =>
				notify(`Erreur isolation des plugins : ${(e as Error)?.message ?? String(e)}`, 'error')
			);
		});
		// Reopens the enabled MIDI keyboards whenever the preference changes or a
		// known one is plugged back in. Chained so overlapping triggers never
		// open/close the same device concurrently.
		let midiSync = Promise.resolve();
		function applyMidiNow() {
			midiSync = midiSync
				.then(() => applyMidiPreferences(get(preferences).midi))
				.then((failed) => {
					if (failed.length) notify(`Clavier MIDI indisponible : ${failed.join(', ')}`, 'error');
				})
				.catch((e) => notify(`Erreur MIDI : ${(e as Error)?.message ?? String(e)}`, 'error'));
		}
		// Skips re-applying when an unrelated preference (audio, project, …)
		// changed. A hotplug event below always re-applies: the device set
		// changed even though the preference didn't.
		let appliedMidi = '';
		const unsubMidi = preferences.subscribe(({ midi }) => {
			const key = JSON.stringify(midi);
			if (key === appliedMidi) return;
			appliedMidi = key;
			applyMidiNow();
		});
		void listMidiInputDevices().then(midiDeviceList.set);
		const unlistenMidiDevices = listen<MidiInputDeviceInfo[]>(MIDI_DEVICES_EVENT, ({ payload }) => {
			midiDeviceList.set(payload);
			applyMidiNow();
		});
		const unsubMetronome = metronomeEnabled.subscribe((enabled) => {
			setMetronomeEnabled(enabled).catch(console.error);
		});
		const unsubMonitor = inputMonitorEnabled.subscribe((enabled) => {
			setInputMonitorEnabled(enabled).catch(console.error);
		});
		const unlistenMidiRecording = connectMidiRecording();
		const unlistenAudioRecording = connectAudioRecording();
		const disconnectBridge = connectTransportBridge();
		const disconnectLoop = connectLoopSync();
		const unregisterShortcuts = registerShortcuts();
		void graphSnapshot.connect();

		const syncDeleteTrackMenu = (hasSelectedTrack: boolean) => {
			invoke('sync_menu_track_selection', { hasSelectedTrack }).catch(console.error);
		};
		syncDeleteTrackMenu(false);
		const unsubSelectedTrack = selectedTrack.subscribe((track) => {
			syncDeleteTrackMenu(track.trackId !== null);
		});

		function runExport(format: 'wav' | 'mp3') {
			exportMixdown(get(engineProject).tracks, get(preferences), format).catch((e: unknown) =>
				notify(`Erreur export : ${(e as Error)?.message ?? String(e)}`, 'error')
			);
		}

		const unlistenMenu = Promise.all([
			listen<string>('menu-export', ({ payload }) => {
				if (payload === 'wav') runExport('wav');
				else if (payload === 'mp3') runExport('mp3');
			}),
			listen('open-preferences', () => {
				settingsPanelOpen.set(true);
			}),
			listen('open-project-settings', () => {
				projectSettingsPanelOpen.set(true);
			})
		]);

		function handleGlobalKey(e: KeyboardEvent) {
			if (isEditingText(e.target)) return;

			if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key === ',') {
				e.preventDefault();
				projectSettingsPanelOpen.update((v) => !v);
			} else if ((e.ctrlKey || e.metaKey) && e.key === ',') {
				e.preventDefault();
				settingsPanelOpen.update((v) => !v);
			} else if (!e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
				if (e.key === 'i' || e.key === 'I') {
					e.preventDefault();
					leftPanel.toggle();
				}
			}
		}
		window.addEventListener('keydown', handleGlobalKey);

		return () => {
			unsubLocale();
			unsubShortcuts();
			disconnectBridge();
			disconnectLoop();
			unregisterShortcuts();
			graphSnapshot.disconnect();
			unsubSelectedTrack();
			unsubIsolation();
			unsubMidi();
			void unlistenMidiDevices.then((fn) => fn());
			unsubMetronome();
			unsubMonitor();
			void unlistenMidiRecording.then((fn) => fn());
			void unlistenAudioRecording.then((fn) => fn());
			void unlistenMenu.then((fns) => fns.forEach((fn) => fn()));
			window.removeEventListener('keydown', handleGlobalKey);
		};
	});
</script>

<!--
  engineProject is subscribed via $engineProject so Svelte keeps the store
  alive (and the Tauri listener active) for the full app lifetime.
  The value is not used here directly — components that need it import it.
-->
{#if $engineProject !== undefined}
	<div class="app">
		<MenuBar />
		<div class="workspace">
			<div class="center-area">
				<LeftPanel />
				{#if $viewMode.main === 'arrange'}
					<MainPanel />
				{:else}
					<GraphView />
				{/if}
				<RightPanel />
			</div>
			<BottomPanel />
		</div>
		<StatusBar />
		<SettingsPanel />
		<ProjectSettingsPanel />
	</div>
{/if}
<MissingPluginsDialog />
<CrashSuspectDialog />

<style>
	.app {
		display: flex;
		flex-direction: column;
		height: 100vh;
		overflow: hidden;
	}

	.workspace {
		flex: 1;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		min-height: 0;
	}

	.center-area {
		flex: 1;
		display: flex;
		flex-direction: row;
		overflow: hidden;
		min-height: 0;
	}
</style>
