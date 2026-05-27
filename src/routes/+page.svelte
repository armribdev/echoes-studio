<script lang="ts">
	import { onMount } from 'svelte';
	import MenuBar from '../components/MenuBar.svelte';
	import MainPannel from '../components/MainPannel.svelte';
	import StatusBar from '../components/StatusBar.svelte';
	// Subscribing to engineProject here triggers connect() on the event store,
	// which starts listening to "engine-event" and "project-loaded" channels.
	// The store then stays alive for the entire app lifetime.
	import { engineProject } from '../frontend/store/engineStore';
	import { registerShortcuts } from '../frontend/shortcuts/keymap';
	import { connectTransportBridge } from '../frontend/transport/bridge';

	// Register keyboard shortcuts and transport bridge once for the app lifetime.
	onMount(() => {
		const disconnectBridge = connectTransportBridge();
		const unregisterShortcuts = registerShortcuts();
		return () => {
			disconnectBridge();
			unregisterShortcuts();
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
	<div class="content">
		<MainPannel />
	</div>
	<StatusBar />
</div>
{/if}

<style>
	.app {
		display: flex;
		flex-direction: column;
		/* height: 100vh; */
	}

	.content {
		flex: 1;
	}
</style>
