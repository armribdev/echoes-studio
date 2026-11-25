<script lang="ts">
	import { t } from 'svelte-i18n';
	import { inputMonitorEnabled } from '../stores/audioRecording';
	import { countInEnabled } from '../stores/metronome';
	import { setInputMonitorEnabled } from '../lib/audioRecordingBackend';
	import Menu from './Menu.svelte';
	import MenuItem from './MenuItem.svelte';

	function toggleMonitor() {
		inputMonitorEnabled.update((enabled) => {
			const next = !enabled;
			setInputMonitorEnabled(next).catch(console.error);
			return next;
		});
	}

	function toggleCountIn() {
		countInEnabled.update((enabled) => !enabled);
	}
</script>

<Menu on:close>
	<MenuItem
		checked={$inputMonitorEnabled}
		title={$t('menuBar.monitorTitle')}
		on:click={toggleMonitor}
	>
		{$t('menuBar.monitor')}
	</MenuItem>
	<MenuItem checked={$countInEnabled} title={$t('menuBar.countInTitle')} on:click={toggleCountIn}>
		{$t('menuBar.countIn')}
	</MenuItem>
</Menu>
