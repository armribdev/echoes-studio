<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import CustomSelect from '../CustomSelect.svelte';
	import { t } from 'svelte-i18n';
	import { NOTE_H, TOTAL_PITCHES } from '../../frontend/pianoRoll/coords';
	import { ADD_ROW_HEIGHT, laneKey, type ControlLane } from '../../frontend/pianoRoll/controlLanes';
	import { noteName } from '../../frontend/sampler/pads';
	import { RULER_HEIGHT } from '../../frontend/timeRuler/constants';
	import { pluginKeyNames } from '../../stores/pluginKeyNames';

	export let keysEl: HTMLDivElement | undefined = undefined;
	export let controlLanes: ControlLane[];
	export let laneHeight: number;
	export let laneCandidates: ControlLane[];
	export let showAddRow: boolean;
	export let hasLaneData: (lane: ControlLane) => boolean;

	const BLACK_KEYS = new Set([1, 3, 6, 8, 10]);

	const dispatch = createEventDispatcher<{
		switchLane: { index: number; key: string };
		addLane: string;
		hideLane: number;
		clearLane: ControlLane;
	}>();

	function laneLabel(lane: ControlLane, translate: typeof $t): string {
		const noteCurves: Partial<Record<ControlLane['kind'], string>> = {
			note_pressure: 'pianoRoll.controls.notePressure',
			note_timbre: 'pianoRoll.controls.noteTimbre',
			note_pitch: 'pianoRoll.controls.notePitch'
		};
		const noteCurve = noteCurves[lane.kind];
		if (noteCurve) return translate(noteCurve);
		if (lane.kind === 'pitch_bend') return translate('pianoRoll.controls.pitchBend');
		if (lane.kind === 'channel_pressure') return translate('pianoRoll.controls.aftertouch');
		if (lane.kind === 'poly_pressure') {
			return translate('pianoRoll.controls.polyAftertouch', {
				values: { note: noteName(lane.number) }
			});
		}
		const named: Record<number, string> = {
			1: 'pianoRoll.controls.modulation',
			11: 'pianoRoll.controls.expression',
			64: 'pianoRoll.controls.sustain'
		};
		return named[lane.number]
			? translate(named[lane.number])
			: translate('pianoRoll.controls.controller', { values: { number: lane.number } });
	}

	function laneOptions(
		lanes: ControlLane[],
		withData: (lane: ControlLane) => boolean,
		translate: typeof $t
	) {
		return lanes.map((lane) => ({
			value: laneKey(lane),
			label: `${withData(lane) ? '• ' : ''}${laneLabel(lane, translate)}`
		}));
	}

	let addLaneValue = '';

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
		<div class="keys-lane" style="height: {laneHeight}px">
			<CustomSelect
				size="xs"
				label={$t('pianoRoll.controls.lane')}
				value={laneKey(lane)}
				options={laneOptions([lane, ...laneCandidates], hasLaneData, $t)}
				on:change={(e) => dispatch('switchLane', { index, key: String(e.detail) })}
			/>
			<div class="lane-actions">
				<button
					class="lane-btn"
					disabled={!hasLaneData(lane)}
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
			<CustomSelect
				size="xs"
				label={$t('pianoRoll.controls.add')}
				placeholder={$t('pianoRoll.controls.add')}
				bind:value={addLaneValue}
				options={laneOptions(laneCandidates, hasLaneData, $t)}
				on:change={(e) => {
					dispatch('addLane', String(e.detail));
					addLaneValue = '';
				}}
			/>
		</div>
	{/if}
	<div class="keys-hscroll-spacer"></div>
</div>

<style lang="scss">
	@use '../../variables.scss' as *;
	@use '../../controls.scss' as *;

	.keys-wrapper {
		display: flex;
		flex-direction: column;
		width: 64px;
		flex-shrink: 0;
		border-right: 1px solid $border-subtle;
	}

	.keys-ruler-spacer {
		background: $panel-background-color;
		border-bottom: 1px solid $border-subtle;
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
		background: $panel-background-color;
		border-bottom: 1px solid $border-subtle;
		cursor: pointer;
		transition: background 0.07s;

		&:hover {
			background-image: linear-gradient($surface-hover, $surface-hover);
		}

		&--black {
			background: $background-color;
			border-bottom: 1px solid $border-faint;
			&:hover {
				background-image: linear-gradient($surface-3, $surface-3);
			}
		}

		&--c {
			border-bottom: 1px solid $border-strong;
		}
	}

	.key-label {
		font-size: 9px;
		font-weight: 600;
		letter-spacing: 0.02em;
		color: $text-faint;
		pointer-events: none;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.keys-vel-label {
		height: 72px; // = VEL_H
		flex-shrink: 0;
		border-top: 1px solid $border-subtle;
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.1em;
		color: $text-faint;
		text-transform: uppercase;
	}

	.keys-lane {
		flex-shrink: 0;
		box-sizing: border-box;
		border-top: 1px solid $border-subtle;
		padding: 4px 3px 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.keys-add {
		flex-shrink: 0;
		box-sizing: border-box;
		border-top: 1px solid $border-subtle;
		padding: 2px 3px;
	}

	.lane-actions {
		display: flex;
		justify-content: flex-end;
		gap: 2px;
	}

	.lane-btn {
		@include icon-button(16px);
		color: $text-faint;
		border-radius: $radius-sm;
	}

	.keys-hscroll-spacer {
		height: 8px;
		flex-shrink: 0;
	}
</style>
