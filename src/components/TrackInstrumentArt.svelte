<script lang="ts">
	import type { InstrumentArtKind } from '../frontend/instrumentArt';
	import keysImg from '../assets/instruments/instrument-keys.png';
	import synthImg from '../assets/instruments/instrument-synth.png';
	import drumsImg from '../assets/instruments/instrument-drums.png';
	import bassImg from '../assets/instruments/instrument-bass.png';
	import guitarImg from '../assets/instruments/instrument-guitar.png';
	import stringsImg from '../assets/instruments/instrument-strings.png';
	import vocalsImg from '../assets/instruments/instrument-vocals.png';
	import padImg from '../assets/instruments/instrument-pad.png';
	import fxImg from '../assets/instruments/instrument-fx.png';

	export let kind: InstrumentArtKind = 'synth';
	export let color: string;

	const ART: Record<InstrumentArtKind, string> = {
		keys: keysImg,
		synth: synthImg,
		drums: drumsImg,
		bass: bassImg,
		guitar: guitarImg,
		strings: stringsImg,
		vocals: vocalsImg,
		pad: padImg,
		fx: fxImg,
	};

	$: src = ART[kind] ?? ART.synth;
</script>

<div class="art" style="--art-color: {color}" data-kind={kind} aria-hidden="true">
	<img class="photo" {src} alt="" draggable="false" />
</div>

<style lang="scss">
	.art {
		width: 40px;
		height: 40px;
		flex-shrink: 0;
		display: grid;
		place-items: center;
		background: transparent;
	}

	.photo {
		width: 100%;
		height: 100%;
		object-fit: contain;
		display: block;
		pointer-events: none;
		user-select: none;
		filter:
			drop-shadow(0 0 0.6px rgba(255, 255, 255, 0.35))
			drop-shadow(0 1px 2px rgba(0, 0, 0, 0.4));
	}
</style>
