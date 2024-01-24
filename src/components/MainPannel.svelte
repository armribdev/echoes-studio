<script lang="ts">
	import Knob from './Knob.svelte';
	import Track from './Track.svelte';
	import type { Track as TrackType } from '../types/global';
	import { invoke } from '@tauri-apps/api/tauri';

	let tracks: TrackType[] = [
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
				{ start: 12, end: 50, raw_samples: [0.5, 0.4, -1, -0.1], sample_rate: 0.5 },
				{
					start: 56,
					end: 87,
					raw_samples: [0.5, 0.4, -1, -0.1, 0.5, 0.4, -1, -0.1, 0.5, 0.4, -1, -0.1],
					sample_rate: 1
				}
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
				console.log('Résultat: ', result);
				let new_clip = {
					start: 0,
					end: result[0].length / result[1],
					raw_samples: result[0],
					sample_rate: result[1]
				};
				tracks[0].clips = [...tracks[0].clips, new_clip];
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
			<svelte:component this={Track} {clips} {scale} />
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

	.instrument {
		height: 100px;
	}

	.instrument {
		padding: 10px;
	}
</style>
