<script lang="ts">
	import Knob from './Knob.svelte';
	import Track from './Track.svelte';
	import type { Track as TrackType, Clip } from '../types/global';
	import { invoke } from '@tauri-apps/api/tauri';
	import { open } from '@tauri-apps/api/dialog';
	import { onMount } from 'svelte';
	import { transport } from '../stores/transport';

	let tracks: TrackType[] = [
		{ name: 'Piano', volume: 60, clips: [] },
		{ name: 'Guitare', volume: 80, clips: [] }
	];

	let scale: number = 2.0;

	// ── Audio engine ──────────────────────────────────────────────────────────
	let audioCtx: AudioContext | null = null;
	let activeSources: AudioBufferSourceNode[] = [];
	let audioStartTime = 0;
	let playbackStartPos = 0;

	function getAudioCtx(): AudioContext {
		if (!audioCtx) audioCtx = new AudioContext();
		return audioCtx;
	}

	function scheduleClip(ctx: AudioContext, clip: Clip, currentPos: number) {
		if (currentPos >= clip.end || clip.sample_rate < 3000) return;

		const samples = new Float32Array(clip.raw_samples);
		const buffer = ctx.createBuffer(1, samples.length, clip.sample_rate);
		buffer.copyToChannel(samples, 0);

		const source = ctx.createBufferSource();
		source.buffer = buffer;
		source.connect(ctx.destination);

		if (currentPos < clip.start) {
			source.start(ctx.currentTime + (clip.start - currentPos));
		} else {
			source.start(0, currentPos - clip.start);
		}

		activeSources.push(source);
	}

	function stopAllClips() {
		for (const source of activeSources) {
			try { source.stop(); } catch { /* already ended */ }
		}
		activeSources = [];
	}

	// ── Playhead (position driven by audioCtx.currentTime when playing) ──────
	let animFrameId: number;

	function loop() {
		if ($transport.isPlaying && audioCtx) {
			transport.update((t) => ({
				...t,
				position: playbackStartPos + (audioCtx!.currentTime - audioStartTime)
			}));
		}
		animFrameId = requestAnimationFrame(loop);
	}

	onMount(() => {
		animFrameId = requestAnimationFrame(loop);

		let wasPlaying = false;
		const unsub = transport.subscribe((state) => {
			if (state.isPlaying && !wasPlaying) {
				const ctx = getAudioCtx();
				if (ctx.state === 'suspended') ctx.resume();
				playbackStartPos = state.position;
				audioStartTime = ctx.currentTime;
				for (const track of tracks) {
					for (const clip of track.clips) {
						scheduleClip(ctx, clip, state.position);
					}
				}
			} else if (!state.isPlaying && wasPlaying) {
				stopAllClips();
			}
			wasPlaying = state.isPlaying;
		});

		return () => {
			cancelAnimationFrame(animFrameId);
			unsub();
			stopAllClips();
		};
	});

	$: playheadX = $transport.position * scale;

	function handleTracksClick(event: MouseEvent) {
		const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
		const x = event.clientX - rect.left;
		transport.update((t) => ({ ...t, position: x / scale }));
	}

	function handleDrop(event: DragEvent) { event.preventDefault(); }
	function handleDragOver(event: DragEvent) { event.preventDefault(); }
	function handleDragEnter(event: DragEvent) { event.preventDefault(); }

	function createNewAudioClip(filePath: string): Promise<[number[], number]> {
		return invoke('create_new_audio_clip', { filePath });
	}

	async function onClick() {
		const filePath = await open({
			filters: [{ name: 'Audio', extensions: ['wav'] }]
		});
		if (!filePath || Array.isArray(filePath)) return;

		createNewAudioClip(filePath as string)
			.then(([samples, sampleRate]) => {
				const clip: Clip = {
					start: 0,
					end: samples.length / sampleRate,
					raw_samples: samples,
					sample_rate: sampleRate
				};
				tracks[0].clips = [...tracks[0].clips, clip];
			})
			.catch((error) => console.error('Erreur:', error));
	}

	function makeEditable(this: HTMLElement) {
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
