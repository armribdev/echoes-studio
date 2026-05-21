<script lang="ts">
	import Knob from './Knob.svelte';
	import Track from './Track.svelte';
	import type { Track as TrackType } from '../types/global';
	import { invoke } from '@tauri-apps/api/tauri';
	import { onMount } from 'svelte';
	import { transport } from '../stores/transport';

	let hoveringAudio: HTMLAudioElement;

	let tracks: TrackType[] = [
		{
			name: 'Piano',
			volume: 60,
			clips: [
				// { start: 109, end: 169 },
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

	// Playhead animation
	let animFrameId: number;
	let lastTs: number | null = null;

	function loop(ts: number) {
		if ($transport.isPlaying) {
			if (lastTs !== null) {
				const delta = (ts - lastTs) / 1000;
				transport.update((t) => ({ ...t, position: t.position + delta }));
			}
			lastTs = ts;
		} else {
			lastTs = null;
		}
		animFrameId = requestAnimationFrame(loop);
	}

	onMount(() => {
		animFrameId = requestAnimationFrame(loop);
		return () => cancelAnimationFrame(animFrameId);
	});

	$: playheadX = $transport.position * scale;

	function handleTracksClick(event: MouseEvent) {
		const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
		const x = event.clientX - rect.left;
		transport.update((t) => ({ ...t, position: x / scale }));
	}

	function handleDrop(event: DragEvent) {
		event.preventDefault();
		console.log('Drop');
	}

	function handleDragOver(event: DragEvent) {
		event.preventDefault();
		console.log('Drag over');
	}

	function handleDragEnter(event: DragEvent) {
		event.preventDefault();
		console.log('Drag enter');
		let files = event.dataTransfer?.files;
		console.log('Data', event.dataTransfer);
		console.log(files?.length);
		if (files?.length) {
			console.log(files?.length > 0);
			if (files?.length > 0) {
				console.log(files[0].type.startsWith('audio/'));
			}
		}

		if (files?.length && files?.length > 0 && files[0].type.startsWith('audio/')) {
			console.log('Filename : ', files[0].name);
			hoveringAudio = new Audio(files[0].name);
		} else {
			console.log('Non');
		}
	}

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

	function makeEditable(this: HTMLElement, event: MouseEvent) {
		this.contentEditable = 'true';
		this.focus();

		this.addEventListener('keydown', function (event) {
			if (event.key === 'Enter') {
				event.preventDefault();
				this.blur();
			}
		});

		this.addEventListener('blur', function () {
			this.contentEditable = 'false';
		});
	}
</script>

<div class="container">
	<input type="range" bind:value={scale} min={0.1} max={10} step={0.01} class="scale" />
	<div class="instruments">
		{#each tracks as { name, volume }}
			<div class="instrument">
				<strong on:dblclick={makeEditable}>{name}</strong>
				<Knob min={0} max={100} bind:value={volume} />
			</div>
		{/each}
	</div>

	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div
		class="tracks"
		on:drop={handleDrop}
		on:dragover={handleDragOver}
		on:dragenter={handleDragEnter}
		on:click={handleTracksClick}
	>
		<div class="playhead" style="transform: translateX({playheadX}px)"></div>
		{#each tracks as { clips }}
			<Track {clips} {scale} />
		{/each}
		<button on:click|stopPropagation={onClick}>Test</button>
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
		background-color: $pannel-background-color;
		width: 200px;
	}

	.tracks {
		flex-grow: 1;
		overflow-y: auto;
		// background-color: blue;
		position: relative;
		cursor: crosshair;
	}

	.instrument {
		height: 100px;
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 10px;
		box-sizing: border-box;
	}

	.playhead {
		position: absolute;
		top: 0;
		left: 0;
		width: 2px;
		height: 100%;
		background-color: #fff;
		pointer-events: none;
		z-index: 10;
		will-change: transform;

		&::before {
			content: '';
			position: absolute;
			top: 0;
			left: -5px;
			width: 0;
			height: 0;
			border-left: 6px solid transparent;
			border-right: 6px solid transparent;
			border-top: 8px solid #fff;
		}
	}
</style>
