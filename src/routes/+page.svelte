<script lang="ts">
	import { onMount } from 'svelte';
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
	import { settingsPanelOpen } from '../stores/preferences';
	import { leftPanel } from '../stores/leftPanel';
	import { rightPanel } from '../stores/rightPanel';

	// Register keyboard shortcuts and transport bridge once for the app lifetime.
	onMount(() => {
		const disconnectBridge = connectTransportBridge();
		const unregisterShortcuts = registerShortcuts();

		function handleGlobalKey(e: KeyboardEvent) {
			if (isEditingText(e.target)) return;

			if ((e.ctrlKey || e.metaKey) && e.key === ',') {
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
			<MainPanel />
			<RightPanel />
		</div>
		<BottomPanel />
	</div>
	<StatusBar />
	<SettingsPanel />
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
