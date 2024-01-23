<script lang="ts">
	let isPlaying = false;
	let isRecording = false;

	function togglePlayPause() {
		isPlaying = !isPlaying;
		// Logique de lecture/pause ici
	}

	function rewindToStart() {
		// Logique de retour au début ici
	}

	function toggleRecord() {
		isRecording = !isRecording;
		// Logique d'enregistrement ici
	}
</script>

<div class="menu-bar">
	<button on:click={togglePlayPause}>{isPlaying ? 'Pause' : 'Play'}</button>
	<button on:click={rewindToStart}>Rewind</button>
	<button on:click={toggleRecord}>{isRecording ? 'Stop Recording' : 'Record'}</button>
</div>

<style lang="scss">
	@import '../variables.scss';
	.menu-bar {
		background-color: $pannel-background-color;
	}
</style>
