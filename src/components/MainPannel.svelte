<script lang="ts">
	import Knob from './Knob.svelte';
	import { invoke } from '@tauri-apps/api/tauri';

	interface Track {
		name: string;
		volume: number;
		clips: { start: number; end: number }[];
	}

	let tracks: Track[] = [
		{
			name: 'Piano',
			volume: 60,
			clips: [
				// { start: 65, end: 89 },
				// { start: 0, end: 65 }
			]
		},
		{
			name: 'Guitare',
			volume: 80,
			clips: [
				{ start: 12, end: 50 },
				{ start: 56, end: 87 }
			]
		}
	];

	let scale: number = 2.0;

	function createNewAudioClip(filePath: string): Promise<[number[], number]> {
		return invoke('create_new_audio_clip', {
			filePath: filePath
		});
	}

	function onClick() {
		const filePath = '/home/armand/dev/echoes-studio/sample_44100.wav';
		createNewAudioClip(filePath)
			.then((result) => {
				console.log('Rsultat: ', result);
				tracks[0].clips.push({ start: 0, end: 100 });
			})
			.catch((error) => {
				console.error('Erreur:', error);
			});
	}
</script>

<div class="container">
	<input type="range" bind:value={scale} min={0.1} max={10} step={0.01} class="scale" />
	<div class="instruments">
		{#each tracks as { name, volume }}
			<div class="instrument">
				<strong>{name}</strong>
				<Knob min={0} max={100} bind:value={volume} />
			</div>
		{/each}
	</div>

	<div class="tracks">
		{#each tracks as { clips }}
			<div class="track">
				{#each clips as { start, end }}
					<div class="clip" style="left: {start * scale}px; width:{(end - start) * scale}px"></div>
				{/each}
			</div>
		{/each}
		<button on:click={onClick}>Test</button>
	</div>
</div>

<style lang="scss">
	@import '../variables.scss';
	.container {
		display: flex;
		align-items: stretch;
		height: 100%;
		position: relative;
		overflow-y: auto;
		overflow-x: hidden;
	}

	.scale {
		position: absolute;
		right: 5px;
		bottom: 5px;
	}

	.instruments {
		border-right: 1px solid #ccc;
		background-color: $pannel-background-color;
	}

	.tracks {
		flex-grow: 1;
		overflow-y: auto;
	}

	.instrument,
	.track {
		height: 100px;
	}

	.instrument {
		padding: 10px;
	}

	.track {
		position: relative;
		// flex: 1;
		// overflow-y: auto;
	}

	.clip {
		position: absolute;
		height: 100%;
		background-color: pink;
		border-radius: 5px;
	}
</style>
