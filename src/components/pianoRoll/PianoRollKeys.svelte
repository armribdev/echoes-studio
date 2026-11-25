<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';
	import { NOTE_H, TOTAL_PITCHES } from '../../frontend/pianoRoll/coords';
	import {
		ADD_ROW_HEIGHT,
		LANE_HEIGHT,
		laneKey,
		lanePoints,
		type ControlLane
	} from '../../frontend/pianoRoll/controlLanes';
	import { noteName } from '../../frontend/sampler/pads';
	import { RULER_HEIGHT } from '../../frontend/timeRuler/constants';
	import { pluginKeyNames } from '../../stores/pluginKeyNames';
	import type { MidiControlPoint } from '../../types/global';

	export let keysEl: HTMLDivElement | undefined = undefined;
	export let controlLanes: ControlLane[];
	export let laneCandidates: ControlLane[];
	export let showAddRow: boolean;
	export let clipControls: MidiControlPoint[];

	const BLACK_KEYS = new Set([1, 3, 6, 8, 10]);

	const dispatch = createEventDispatcher<{
		switchLane: { index: number; key: string };
		addLane: string;
		hideLane: number;
		clearLane: ControlLane;
	}>();

	function laneLabel(lane: ControlLane, translate: typeof $t): string {
		if (lane.kind === 'pitch_bend') return translate('pianoRoll.controls.pitchBend');
		if (lane.kind === 'channel_pressure') return translate('pianoRoll.controls.aftertouch');
		const named: Record<number, string> = {
			1: 'pianoRoll.controls.modulation',
			11: 'pianoRoll.controls.expression',
			64: 'pianoRoll.controls.sustain'
		};
		return named[lane.number]
			? translate(named[lane.number])
			: translate('pianoRoll.controls.controller', { values: { number: lane.number } });
	}

	function laneOptionLabel(lane: ControlLane, controls: MidiControlPoint[], translate: typeof $t) {
		const marker = lanePoints(controls, lane).length > 0 ? '• ' : '';
		return `${marker}${laneLabel(lane, translate)}`;
	}

	function getPianoKeyLabel(pitch: number): string {
		const deg = pitch % 12;
		const oct = Math.floor(pitch / 12) - 1;
		if (deg === 0) return `C${oct}`;
		if (deg === 4) return `E${oct}`;
		if (deg === 9) return `A${oct}`;
		return '';
	}

	function isBlack(pitch: number): boolean {
		return BLACK_KEYS.has(pitch % 12);
	}
</script>

<div class="keys-wrapper">
	<div class="keys-ruler-spacer" style="height: {RULER_HEIGHT}px"></div>
	<div class="keys-scroll" bind:this={keysEl} role="list" aria-label={$t('pianoRoll.keys')}>
		{#each { length: TOTAL_PITCHES } as _, i}
			{@const pitch = TOTAL_PITCHES - 1 - i}
			{@const pluginName = $pluginKeyNames.get(pitch)}
			<div
				class="key"
				class:key--black={isBlack(pitch)}
				class:key--c={pitch % 12 === 0}
				style="height: {NOTE_H}px"
				title={pluginName ? `${pluginName} · ${noteName(pitch)}` : undefined}
				role="listitem"
			>
				<span class="key-label">{pluginName ?? getPianoKeyLabel(pitch)}</span>
			</div>
		{/each}
	</div>
	<div class="keys-vel-label">VEL</div>
	{#each controlLanes as lane, index (laneKey(lane))}
		<div class="keys-lane" style="height: {LANE_HEIGHT}px">
			<select
				class="lane-select"
				value={laneKey(lane)}
				aria-label={$t('pianoRoll.controls.lane')}
				on:change={(e) => dispatch('switchLane', { index, key: e.currentTarget.value })}
			>
				{#each [lane, ...laneCandidates] as option (laneKey(option))}
					<option value={laneKey(option)}>
						{laneOptionLabel(option, clipControls, $t)}
					</option>
				{/each}
			</select>
			<div class="lane-actions">
				<button
					class="lane-btn"
					disabled={lanePoints(clipControls, lane).length === 0}
					title={$t('pianoRoll.controls.clear')}
					aria-label={$t('pianoRoll.controls.clear')}
					on:click={() => dispatch('clearLane', lane)}
				>
					<svg viewBox="0 0 10 10" width="10" height="10" fill="none" aria-hidden="true">
						<path
							d="M1.5 3h7M4 3V1.8h2V3M2.6 3l.4 5.2h4l.4-5.2"
							stroke="currentColor"
							stroke-width="1.1"
							stroke-linecap="round"
							stroke-linejoin="round"
						/>
					</svg>
				</button>
				<button
					class="lane-btn"
					title={$t('pianoRoll.controls.hide')}
					aria-label={$t('pianoRoll.controls.hide')}
					on:click={() => dispatch('hideLane', index)}
				>
					<svg viewBox="0 0 10 10" width="10" height="10" fill="none" aria-hidden="true">
						<path
							d="M2 2L8 8M8 2L2 8"
							stroke="currentColor"
							stroke-width="1.2"
							stroke-linecap="round"
						/>
					</svg>
				</button>
			</div>
		</div>
	{/each}
	{#if showAddRow}
		<div class="keys-add" style="height: {ADD_ROW_HEIGHT}px">
			<select
				class="lane-select"
				value=""
				aria-label={$t('pianoRoll.controls.add')}
				on:change={(e) => {
					dispatch('addLane', e.currentTarget.value);
					e.currentTarget.value = '';
				}}
			>
				<option value="">{$t('pianoRoll.controls.add')}</option>
				{#each laneCandidates as option (laneKey(option))}
					<option value={laneKey(option)}>
						{laneOptionLabel(option, clipControls, $t)}
					</option>
				{/each}
			</select>
		</div>
	{/if}
	<div class="keys-hscroll-spacer"></div>
</div>

<style lang="scss">
	.keys-wrapper {
		display: flex;
		flex-direction: column;
		width: 64px;
		flex-shrink: 0;
		border-right: 1px solid #2a2a38;
	}

	.keys-ruler-spacer {
		background: #1d1d28;
		border-bottom: 1px solid #2a2a38;
		flex-shrink: 0;
	}

	.keys-scroll {
		flex: 1;
		overflow: hidden;
		display: flex;
		flex-direction: column;
	}

	.key {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		padding-right: 5px;
		box-sizing: border-box;
		flex-shrink: 0;
		background: #252535;
		border-bottom: 1px solid #1c1c28;
		cursor: pointer;
		transition: background 0.07s;

		&:hover {
			background: #303048;
		}

		&--black {
			background: #17171e;
			border-bottom: 1px solid #101015;
			&:hover {
				background: #21212e;
			}
		}

		&--c {
			border-bottom: 1px solid #32323e;
		}
	}

	.key-label {
		font-size: 9px;
		font-weight: 600;
		letter-spacing: 0.02em;
		color: #4a4a60;
		pointer-events: none;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.keys-vel-label {
		height: 72px; // = VEL_H
		flex-shrink: 0;
		border-top: 1px solid #2a2a38;
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.1em;
		color: #383848;
		text-transform: uppercase;
	}

	.keys-lane {
		flex-shrink: 0;
		box-sizing: border-box;
		border-top: 1px solid #2a2a38;
		padding: 4px 3px 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.keys-add {
		flex-shrink: 0;
		box-sizing: border-box;
		border-top: 1px solid #2a2a38;
		padding: 2px 3px;
	}

	.lane-select {
		width: 100%;
		min-width: 0;
		height: 18px;
		box-sizing: border-box;
		padding: 0 2px;
		font-size: 9px;
		font-weight: 600;
		color: #8a8aa0;
		background: #20202c;
		border: 1px solid #2e2e3c;
		border-radius: 3px;
		text-overflow: ellipsis;

		&:focus-visible {
			outline: 1px solid var(--track-color, #4d96ff);
		}
	}

	.lane-actions {
		display: flex;
		justify-content: flex-end;
		gap: 2px;
	}

	.lane-btn {
		width: 18px;
		height: 16px;
		padding: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		color: #5a5a70;
		background: transparent;
		border: none;
		border-radius: 3px;
		cursor: pointer;

		&:hover:not(:disabled) {
			color: #c8c8d8;
			background: #2a2a38;
		}

		&:disabled {
			opacity: 0.35;
			cursor: default;
		}

		&:focus-visible {
			outline: 1px solid var(--track-color, #4d96ff);
		}
	}

	.keys-hscroll-spacer {
		height: 8px;
		flex-shrink: 0;
	}
</style>
