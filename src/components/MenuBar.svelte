<script lang="ts">
	import { transport } from '../stores/transport';

	let isRecording = false;

	function togglePlayPause() {
		transport.update((t) => ({ ...t, isPlaying: !t.isPlaying }));
	}

	function rewindToStart() {
		transport.update((t) => ({ ...t, position: 0, isPlaying: false }));
	}

	function toggleRecord() {
		isRecording = !isRecording;
	}
</script>

<div class="menu-bar">
	<button on:click={togglePlayPause}>{$transport.isPlaying ? 'Pause' : 'Play'}</button>
	<button on:click={rewindToStart}>Rewind</button>
	<button on:click={toggleRecord}>{isRecording ? 'Stop Recording' : 'Record'}</button>
</div>

<style lang="scss">
	@import '../variables.scss';
	.menu-bar {
		background-color: $pannel-background-color;
	}
</style>
