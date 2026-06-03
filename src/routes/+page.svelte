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
	import { registerShortcuts } from '../frontend/shortcuts/keymap';
	import { isEditingText } from '../frontend/shortcuts/context';
	import { connectTransportBridge } from '../frontend/transport/bridge';
	import SettingsPanel from '../components/SettingsPanel.svelte';
	import ProjectSettingsPanel from '../components/ProjectSettingsPanel.svelte';
	import GraphView from '../components/GraphView.svelte';
	import { settingsPanelOpen, preferences } from '../stores/preferences';
	import { projectSettingsPanelOpen } from '../stores/project';
	import { exportMixdown } from '../lib/exportMixdown';
	import { notify } from '../stores/status';
	import { leftPanel } from '../stores/leftPanel';
	import { rightPanel } from '../stores/rightPanel';
	import { viewMode } from '../stores/viewMode';
	import { graphSnapshot } from '../stores/graphSnapshot';
	import { selectedTrack } from '../stores/selectedTrack';

	// Register keyboard shortcuts and transport bridge once for the app lifetime.
	onMount(() => {
		const disconnectBridge = connectTransportBridge();
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
			}),
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
				} else if (e.key === 'l' || e.key === 'L') {
					e.preventDefault();
					rightPanel.toggle();
				}
			}
		}
		window.addEventListener('keydown', handleGlobalKey);

		return () => {
			disconnectBridge();
			unregisterShortcuts();
			graphSnapshot.disconnect();
			unsubSelectedTrack();
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
