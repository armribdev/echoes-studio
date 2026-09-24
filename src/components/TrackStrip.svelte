<script lang="ts">
	import FaderMeter from './FaderMeter.svelte';
	import Knob from './Knob.svelte';
	import { createEventDispatcher } from 'svelte';
	import type { Track } from '../types/global';
	import { modulatorColor } from '../frontend/modulation/colors';

	export let track: Track;
	export let selected: boolean = false;
	export let color: string;
	export let peakL: number = 0;
	export let peakR: number = 0;

	// Two-way bindable so the parent array stays in sync during drag
	export let volume: number;
	export let pan: number;

	let volDrag = false;
	let localVolume = volume;
	$: if (!volDrag) localVolume = volume;

	$: volumeModColor = trackGainModColor(track);

	function trackGainModColor(t: Track): string | undefined {
		const route = t.mod_routes?.find((r) => r.target.kind === 'track_gain');
		if (!route) return undefined;
		const index = t.modulators?.findIndex((m) => m.id === route.modulator_id) ?? -1;
		return modulatorColor(index >= 0 ? index : 0);
	}

	function onVolumeChange(value: number) {
		volDrag = true;
		localVolume = value;
		dispatch('volumechange', value);
	}

	function onVolumeRelease(value: number) {
		volDrag = false;
		localVolume = value;
		dispatch('volumerelease', value);
	}

	const dispatch = createEventDispatcher<{
		select: void;
		contextmenu: MouseEvent;
		mutetoggle: void;
		solotoggle: void;
		volumerelease: number;
		volumechange: number;
		panrelease: void;
		panchange: void;
		rename: { name: string };
		openpianoroll: void;
	}>();

	function makeEditable(e: MouseEvent) {
		const el = e.currentTarget;
		if (!(el instanceof HTMLElement)) return;

		el.contentEditable = 'true';
		el.focus();

		const range = document.createRange();
		range.selectNodeContents(el);
		const sel = window.getSelection();
		sel?.removeAllRanges();
		sel?.addRange(range);

		el.addEventListener('keydown', function (event) {
			if (event.key === 'Enter') {
				event.preventDefault();
				el.blur();
			}
		});

		el.addEventListener('blur', function () {
			el.contentEditable = 'false';
			const newName = el.textContent?.trim() || track.name;
			dispatch('rename', { name: newName });
		}, { once: true });
	}
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	class="instrument"
	class:instrument--selected={selected}
	style="--track-color: {color}"
	on:mousedown={() => dispatch('select')}
	on:contextmenu={(e) => { e.preventDefault(); e.stopPropagation(); dispatch('contextmenu', e); }}
>
	<div class="track-head">
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<strong class="track-name" on:dblclick={makeEditable}>{track.name}</strong>
		<button
			type="button"
			class="ms-btn"
			class:active={track.muted}
			title="Mute"
			on:click|stopPropagation={() => dispatch('mutetoggle')}
		>M</button>
		<button
			type="button"
			class="ms-btn"
			class:active={track.solo}
			title="Solo"
			on:click|stopPropagation={() => dispatch('solotoggle')}
		>S</button>
	</div>

	<div class="track-controls">
		<div class="ctrl-row">
			<div class="ctrl-pan">
				<span class="ctrl-label">PAN</span>
				<Knob
					min={-100}
					max={100}
					defaultValue={0}
					bind:value={pan}
					on:change={() => dispatch('panchange')}
					on:release={() => dispatch('panrelease')}
				/>
			</div>
			<div class="ctrl-vol">
				<span class="ctrl-label">VOL</span>
				<FaderMeter
					bind:value={localVolume}
					{peakL}
					{peakR}
					defaultValue={75}
					modColor={volumeModColor}
					modTargetKey={selected ? 'track_gain' : undefined}
					on:change={(e) => onVolumeChange(e.detail)}
					on:release={(e) => onVolumeRelease(e.detail)}
				/>
			</div>
		</div>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.instrument {
		height: 100px;
		display: flex;
		flex-direction: column;
		box-sizing: border-box;
		border-bottom: 1px solid $border-color;
		position: relative;
		transition: background 0.1s;

		&::before {
			content: '';
			position: absolute;
			left: 0;
			top: 10px;
			bottom: 10px;
			width: 3px;
			border-radius: 0 2px 2px 0;
			background: var(--track-color, #{$accent-color});
		}

		&--selected { background: $surface-1; }
	}

	.track-head {
		height: 32px;
		flex-shrink: 0;
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 0 6px 0 13px;
		border-bottom: 1px solid rgba(255, 255, 255, 0.04);
	}

	.track-name {
		flex: 1;
		font-size: 11px;
		font-weight: 500;
		color: $text-color;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		min-width: 0;
		cursor: default;

		&:focus { outline: none; }
	}

	.ms-btn {
		width: 20px;
		height: 20px;
		flex-shrink: 0;
		padding: 0;
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: 3px;
		background: transparent;
		color: rgba($text-color, 0.45);
		font-size: 10px;
		font-weight: 700;
		line-height: 1;
		cursor: pointer;
		transition: background 0.1s, color 0.1s, border-color 0.1s;

		&:hover {
			color: $text-color;
			border-color: rgba($text-color, 0.35);
		}

		&.active {
			background: $accent-color;
			border-color: $accent-color;
			color: #fff;
		}
	}

	.track-controls {
		flex: 1;
		display: flex;
		align-items: center;
		padding: 0 8px 4px 13px;
		gap: 0;
	}

	.ctrl-row {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.ctrl-label {
		font-size: 8px;
		font-family: $font-mono;
		color: $text-faint;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		line-height: 1;
		user-select: none;
	}

	.ctrl-pan {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 3px;
		flex-shrink: 0;
	}

	.ctrl-vol {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 5px;
		min-width: 0;
	}
</style>
