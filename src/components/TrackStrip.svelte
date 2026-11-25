<script lang="ts">
	import FaderMeter from './FaderMeter.svelte';
	import Knob from './Knob.svelte';
	import TrackInstrumentArt from './TrackInstrumentArt.svelte';
	import { createEventDispatcher } from 'svelte';
	import type { Track } from '../types/global';
	import { modulatorColor } from '../frontend/modulation/colors';
	import { automatedTargets, automatedValues } from '../stores/automation';
	import { TRACK_COLORS } from '../frontend/trackColor';
	import { instrumentArtKind } from '../frontend/instrumentArt';
	import { instrumentDisplayName, plugins } from '../stores/plugins';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';
	import { t } from 'svelte-i18n';

	export let track: Track;
	export let selected: boolean = false;
	export let armed: boolean = false;
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
	$: automatedGain = $automatedValues[track.id]?.['track_gain'];
	$: automatedVolume = automatedGain === undefined ? undefined : automatedGain * 100;
	$: artKind = instrumentArtKind(
		track.instrument,
		instrumentDisplayName($plugins, track.instrument)
	);

	let colorOpen = false;

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
		armtoggle: void;
		volumerelease: number;
		volumechange: number;
		panrelease: void;
		panchange: void;
		rename: { name: string };
		openpianoroll: void;
		colorchange: string;
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

		el.addEventListener(
			'blur',
			function () {
				el.contentEditable = 'false';
				const newName = el.textContent?.trim() || track.name;
				dispatch('rename', { name: newName });
			},
			{ once: true }
		);
	}

	function pickColor(next: string) {
		colorOpen = false;
		if (next === color) return;
		dispatch('colorchange', next);
	}
</script>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	class="instrument"
	class:instrument--selected={selected}
	style="--track-color: {color}"
	on:mousedown={() => dispatch('select')}
	on:contextmenu={(e) => {
		e.preventDefault();
		e.stopPropagation();
		dispatch('contextmenu', e);
	}}
>
	<div class="color-slot">
		<button
			type="button"
			class="color-bar"
			style="background: {color}"
			aria-label={$t('trackStrip.changeColor')}
			aria-expanded={colorOpen}
			on:click|stopPropagation={() => {
				colorOpen = !colorOpen;
			}}
		></button>
		{#if colorOpen}
			<div
				class="color-pop"
				role="menu"
				tabindex="-1"
				aria-label={$t('trackStrip.colors')}
				use:focusTrap={{ initial: 'first' }}
				use:dismissable={() => {
					colorOpen = false;
				}}
				on:mousedown|stopPropagation
			>
				{#each TRACK_COLORS as swatch}
					<button
						type="button"
						class="color-opt"
						class:active={swatch === color}
						style="background: {swatch}"
						role="menuitem"
						aria-label={swatch}
						on:click|stopPropagation={() => pickColor(swatch)}
					></button>
				{/each}
			</div>
		{/if}
	</div>

	<div class="track-stack">
		<div class="track-top">
			<div class="art-wrap">
				<TrackInstrumentArt kind={artKind} {color} />
			</div>

			<strong class="track-name" on:dblclick={makeEditable}>{track.name}</strong>
			<div class="ms-group" role="group" aria-label={$t('trackStrip.muteSoloArm')}>
				<button
					type="button"
					class="ms-btn"
					class:active={track.muted}
					title={$t('trackStrip.mute')}
					aria-pressed={track.muted}
					on:click|stopPropagation={() => dispatch('mutetoggle')}>M</button
				>
				<button
					type="button"
					class="ms-btn"
					class:active={track.solo}
					title={$t('trackStrip.solo')}
					aria-pressed={track.solo}
					on:click|stopPropagation={() => dispatch('solotoggle')}>S</button
				>
				<button
					type="button"
					class="ms-btn ms-btn--arm"
					class:active={armed}
					title={$t('trackStrip.arm')}
					aria-pressed={armed}
					aria-label={$t('trackStrip.arm')}
					on:click|stopPropagation={() => dispatch('armtoggle')}
					><span class="arm-dot" aria-hidden="true"></span></button
				>
			</div>
		</div>

		<div class="track-controls">
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
					automationColor={$automatedTargets[track.id]?.has('track_gain') ? color : undefined}
					automatedValue={automatedVolume}
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
		align-items: stretch;
		box-sizing: border-box;
		border-bottom: 1px solid $border-color;
		position: relative;
		transition: background $duration-base;
		padding: 0;
		gap: 0;

		&--selected {
			background: $surface-1;
		}
	}

	.color-slot {
		position: relative;
		flex-shrink: 0;
		width: 10px;
		padding: 10px 0;
		box-sizing: border-box;
	}

	.color-bar {
		display: block;
		width: 3px;
		height: 100%;
		margin: 0;
		padding: 0;
		border: none;
		border-radius: 0 2px 2px 0;
		cursor: pointer;
		transition:
			width $duration-base ease,
			filter $duration-base ease;

		&:hover {
			width: 5px;
			filter: brightness(1.12);
		}
	}

	.color-pop {
		position: absolute;
		z-index: 30;
		left: 10px;
		top: 50%;
		transform: translateY(-50%);
		display: grid;
		grid-template-columns: repeat(6, 20px);
		gap: 5px;
		padding: 8px;
		background: $surface-overlay;
		border: 1px solid $border-color;
		border-radius: $radius-lg;
		box-shadow: $shadow-overlay;
	}

	.color-opt {
		width: 20px;
		height: 20px;
		padding: 0;
		border: 1px solid rgba(255, 255, 255, 0.08);
		border-radius: $radius-md;
		cursor: pointer;
		transition:
			transform $duration-fast ease,
			box-shadow $duration-fast ease;

		&.active {
			box-shadow:
				0 0 0 2px $panel-background-color,
				0 0 0 3.5px #fff;
		}

		&:hover {
			transform: scale(1.08);
			filter: brightness(1.08);
		}
	}

	.track-stack {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		padding: 6px 8px 6px 6px;
		gap: 4px;
	}

	.track-top {
		flex-shrink: 0;
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 0;
	}

	.art-wrap {
		flex-shrink: 0;
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

		&:focus {
			outline: none;
		}
	}

	.ms-group {
		display: inline-flex;
		flex-shrink: 0;
		align-items: stretch;
		height: 20px;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		overflow: hidden;
	}

	.ms-btn {
		width: 20px;
		height: 100%;
		padding: 0;
		border: none;
		border-right: 1px solid $border-subtle;
		border-radius: 0;
		background: transparent;
		color: $text-subtle;
		font-size: 10px;
		font-weight: 700;
		font-family: inherit;
		line-height: 1;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition:
			background $duration-base,
			color $duration-base;

		&:last-child {
			border-right: none;
		}

		&:hover {
			color: $text-color;
			background: $surface-hover;
		}

		&.active {
			background: $accent-color;
			color: #fff;
		}
	}

	.ms-btn--arm {
		.arm-dot {
			width: 6px;
			height: 6px;
			border-radius: 50%;
			background: currentColor;
			opacity: 0.55;
		}

		&.active {
			background: #e5484d;
			color: #fff;

			.arm-dot {
				opacity: 1;
			}
		}

		&:hover .arm-dot {
			opacity: 1;
		}
	}

	.track-controls {
		flex: 1;
		min-height: 0;
		width: 100%;
		display: flex;
		align-items: center;
		gap: 10px;
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
		min-width: 0;
		display: flex;
		flex-direction: column;
		align-items: stretch;
		gap: 5px;
	}
</style>
