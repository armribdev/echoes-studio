<script>
	import Knob from './Knob.svelte';
	let tracks = [
		{
			name: 'Piano',
			volume: 60,
			clips: [
				{ start: 65, end: 89 },
				{ start: 0, end: 65 }
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
</script>

<div class="container">
	<div class="instruments">
		{#each tracks as { name, volume }}
			<div class="instrument">
				<strong>{name}</strong>
				<Knob min="0" max="100" bind:value={volume} />
			</div>
		{/each}
	</div>

	<div class="tracks">
		{#each tracks as { clips }}
			<div class="track">
				{#each clips as { start, end }}
					<div class="clip" style="left: {start}px; width:{end - start}px"></div>
				{/each}
			</div>
		{/each}
	</div>
</div>

<style lang="scss">
	@import '../variables.scss';
	.container {
		display: flex;
		align-items: stretch;
		height: 100%;
	}

	.instruments {
		border-right: 1px solid #ccc;
		background-color: $pannel-background-color;
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
	}

	.clip {
		position: absolute;
		height: 100%;
		background-color: pink;
		border-radius: 5px;
	}
</style>
