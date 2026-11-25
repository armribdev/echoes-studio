<script lang="ts">
	import { t } from 'svelte-i18n';
	import { transport } from '../stores/transport';
	import { loopRegion } from '../stores/loopRegion';
	import { tempo } from '../stores/tempo';
	import { play, pause, stop, seek } from '../frontend/commands/dispatcher';
	import { leftPanel } from '../stores/leftPanel';
	import { rightPanel } from '../stores/rightPanel';
	import { projectSettingsPanelOpen } from '../stores/project';
	import { shortcutLabels } from '../frontend/shortcuts/label';
	import { isMidiRecording } from '../stores/midiRecording';
	import { isAudioRecording } from '../stores/audioRecording';
	import { metronomeEnabled } from '../stores/metronome';
	import { toggleRecording } from '../frontend/audioRecording';
	import Icon from './Icon.svelte';
	import RecordOptionsMenu from './RecordOptionsMenu.svelte';

	$: isRecording = $isMidiRecording || $isAudioRecording;

	let recordOptionsOpen = false;

	function togglePlayPause() {
		if ($transport.isPlaying) {
			pause().catch(console.error);
		} else {
			play().catch(console.error);
		}
	}

	function rewindToStart() {
		seek(0)
			.then(() => stop())
			.catch(console.error);
	}

	function formatBarBeat(totalBeats: number, beatsPerBar: number): string {
		const bar = Math.floor(totalBeats / beatsPerBar) + 1;
		const beat = Math.floor(totalBeats % beatsPerBar) + 1;
		return `${bar}.${beat}`;
	}

	$: loopStartLabel = formatBarBeat($loopRegion.startBeat, $tempo.numerator);
	$: loopEndLabel = formatBarBeat($loopRegion.endBeat, $tempo.numerator);

	// Live position in bar.beat format
	let barStr = '001';
	let beatStr = '1';

	$: {
		const beatSec = 60 / $tempo.bpm;
		const barSec = $tempo.numerator * beatSec;
		const bar = Math.floor($transport.position / barSec) + 1;
		const beat = Math.floor(($transport.position % barSec) / beatSec) + 1;
		barStr = String(bar).padStart(3, '0');
		beatStr = String(beat);
	}
</script>

<div class="menu-bar">
	<div class="zone zone--left">
		<button
			class="btn btn--panel-toggle"
			class:active={$leftPanel.open}
			on:click={() => leftPanel.toggle()}
			title={$t('menuBar.inspectorTitle')}
			aria-label={$t('menuBar.inspector')}
		>
			<Icon name="inspector" size={14} />
		</button>
	</div>

	<div class="zone zone--center">
		<div class="transport">
			<button class="btn" on:click={rewindToStart} title={$t('menuBar.rewind')}>
				<svg
					width="13"
					height="13"
					viewBox="0 0 13 13"
					fill="none"
					xmlns="http://www.w3.org/2000/svg"
				>
					<rect x="1" y="1.5" width="1.6" height="10" rx="0.8" fill="currentColor" />
					<path d="M11.5 2L4.5 6.5L11.5 11V2Z" fill="currentColor" />
				</svg>
			</button>
			<button
				class="btn btn--loop"
				class:active={$loopRegion.enabled}
				on:click={() => loopRegion.toggle()}
				title={$t('menuBar.loop')}
				aria-pressed={$loopRegion.enabled}
			>
				<svg
					width="14"
					height="12"
					viewBox="0 0 14 12"
					fill="none"
					xmlns="http://www.w3.org/2000/svg"
				>
					<path d="M3.2 2.2H9.2V0.4L12.6 3.2L9.2 6V4.2H3.2V2.2Z" fill="currentColor" />
					<path d="M10.8 9.8H4.8V11.6L1.4 8.8L4.8 6V7.8H10.8V9.8Z" fill="currentColor" />
				</svg>
			</button>
			<button
				class="btn btn--play"
				on:click={togglePlayPause}
				title={`${$transport.isPlaying ? $t('menuBar.pause') : $t('menuBar.play')} (${$shortcutLabels('transport.playPause')})`}
			>
				{#if $transport.isPlaying}
					<svg
						width="11"
						height="13"
						viewBox="0 0 11 13"
						fill="none"
						xmlns="http://www.w3.org/2000/svg"
					>
						<rect x="1" y="1" width="3" height="11" rx="1" fill="currentColor" />
						<rect x="7" y="1" width="3" height="11" rx="1" fill="currentColor" />
					</svg>
				{:else}
					<svg
						width="12"
						height="13"
						viewBox="0 0 12 13"
						fill="none"
						xmlns="http://www.w3.org/2000/svg"
					>
						<path d="M2 1.5L11 6.5L2 11.5V1.5Z" fill="currentColor" />
					</svg>
				{/if}
			</button>
			<div class="split-button">
				<button
					class="btn btn--record"
					class:active={isRecording}
					on:click={() => toggleRecording().catch(console.error)}
					title={$t('menuBar.record')}
					aria-pressed={isRecording}
				>
					<svg
						width="12"
						height="12"
						viewBox="0 0 12 12"
						fill="none"
						xmlns="http://www.w3.org/2000/svg"
					>
						<circle cx="6" cy="6" r="5" fill="currentColor" />
					</svg>
				</button>
				<button
					class="btn btn--chevron"
					on:click={() => (recordOptionsOpen = true)}
					title={$t('menuBar.recordOptions')}
					aria-label={$t('menuBar.recordOptions')}
					aria-haspopup="menu"
					aria-expanded={recordOptionsOpen}
				>
					<svg
						width="8"
						height="5"
						viewBox="0 0 8 5"
						fill="none"
						xmlns="http://www.w3.org/2000/svg"
					>
						<path
							d="M1 1L4 4L7 1"
							stroke="currentColor"
							stroke-width="1.2"
							stroke-linecap="round"
							stroke-linejoin="round"
						/>
					</svg>
				</button>
				{#if recordOptionsOpen}
					<RecordOptionsMenu on:close={() => (recordOptionsOpen = false)} />
				{/if}
			</div>
			<button
				class="btn btn--toggle btn--option"
				class:active={$metronomeEnabled}
				on:click={() => metronomeEnabled.update((enabled) => !enabled)}
				title={$t('menuBar.metronome')}
				aria-label={$t('menuBar.metronome')}
				aria-pressed={$metronomeEnabled}
			>
				<svg
					width="12"
					height="13"
					viewBox="0 0 12 13"
					fill="none"
					xmlns="http://www.w3.org/2000/svg"
				>
					<path
						d="M3.6 1.5H8.4L11 12H1L3.6 1.5Z"
						stroke="currentColor"
						stroke-width="1.2"
						stroke-linejoin="round"
					/>
					<path d="M6 9L8.6 3.6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
				</svg>
			</button>
		</div>

		<div class="sep"></div>

		<div class="position">
			<span class="ctrl-label">{$t('menuBar.position')}</span>
			<div class="position-display">
				<span class="pos-bar">{barStr}</span>
				<span class="pos-sep">.</span>
				<span class="pos-beat">{beatStr}</span>
			</div>
		</div>

		<div class="sep"></div>

		<button
			class="tempo"
			on:click={() => projectSettingsPanelOpen.set(true)}
			title={$t('menuBar.tempoTitle')}
		>
			<span class="ctrl-label">{$t('menuBar.tempo')}</span>
			<span class="tempo-display">
				<span class="tempo-bpm">{$tempo.bpm}</span>
				<span class="tempo-signature">{$tempo.numerator}/{$tempo.denominator}</span>
			</span>
		</button>

		<div class="loop-range" class:loop-range--hidden={!$loopRegion.enabled}>
			<div class="sep"></div>
			<div class="position">
				<span class="ctrl-label">{$t('menuBar.loopLabel')}</span>
				<span class="tempo-display">
					<span class="tempo-bpm">{loopStartLabel}</span>
					<span class="tempo-signature">→</span>
					<span class="tempo-bpm">{loopEndLabel}</span>
				</span>
			</div>
		</div>
	</div>

	<div class="zone zone--right">
		<button
			class="btn btn--panel-toggle"
			class:active={$rightPanel.open}
			on:click={() => rightPanel.toggle()}
			title={`${$t('menuBar.libraryTitle')} (${$shortcutLabels('library.toggle')})`}
			aria-label={$t('menuBar.library')}
		>
			<Icon name="library" size={14} />
		</button>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.menu-bar {
		background-color: $panel-background-color;
		height: 48px;
		display: grid;
		grid-template-columns: 1fr auto 1fr;
		align-items: center;
		padding: 0 $space-5;
		border-bottom: 1px solid $border-color;
		flex-shrink: 0;
	}

	.zone {
		display: flex;
		align-items: center;
		min-width: 0;

		&--left {
			justify-content: flex-start;
			gap: $space-2;
		}

		&--center {
			justify-content: center;
			gap: $space-5;
		}

		&--right {
			justify-content: flex-end;
			gap: $space-2;
		}
	}

	// ── Transport ─────────────────────────────────────────────────────────────
	.transport {
		display: flex;
		align-items: center;
		gap: $space-2;
	}

	.btn {
		@include icon-button($ctrl-height);
		font-size: 13px;

		&--loop.active,
		&--toggle.active,
		&--panel-toggle.active {
			@include control-on;
		}

		&--play,
		&--record,
		&--chevron,
		&--panel-toggle {
			border-color: $border-subtle;
			background: $surface-2;
			color: $text-secondary;

			&:not(:disabled):hover {
				background: $surface-3;
				border-color: $border-strong;
				color: $text-color;
			}
		}

		&--play {
			width: 38px;
			color: $text-color;
		}

		&--option {
			width: $ctrl-height-sm;
			height: $ctrl-height-sm;

			svg {
				width: 11px;
				height: auto;
			}
		}

		&--chevron {
			width: 18px;
			border-radius: 0 $radius-control $radius-control 0;
		}

		&--record {
			width: 34px;
			border-right: none;
			border-radius: $radius-control 0 0 $radius-control;

			&.active {
				@include control-on-danger;
				border-right: none;
			}
			&:not(:disabled):hover {
				color: $color-error-text;
			}
		}
	}

	.split-button {
		position: relative;
		display: flex;
		align-items: center;

		+ .btn--option {
			margin-left: $space-3;
		}
	}

	.btn--panel-toggle {
		width: $ctrl-height;
	}

	// ── Separator ─────────────────────────────────────────────────────────────
	.sep {
		width: 1px;
		height: 22px;
		background: $border-color;
		flex-shrink: 0;
	}

	.ctrl-label {
		font-size: 9px;
		font-family: $font-mono;
		color: $text-faint;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		line-height: 1;
		user-select: none;
	}

	.position {
		display: flex;
		flex-direction: column;
		gap: $space-1;
	}

	.position-display {
		display: flex;
		align-items: baseline;
		font-family: $font-mono;
		line-height: 1;
	}

	.pos-bar {
		font-size: 24px;
		font-weight: 500;
		color: $text-color;
		letter-spacing: 1px;
	}

	.pos-sep {
		font-size: 19px;
		color: $text-subtle;
		margin: 0 1px;
	}

	.pos-beat {
		font-size: 21px;
		color: $text-secondary;
	}

	.tempo {
		display: flex;
		flex-direction: column;
		gap: $space-1;
		padding: $space-1 $space-3;
		margin: -2px -6px;
		background: none;
		border: none;
		border-radius: $radius-control;
		cursor: pointer;
		text-align: left;
		transition: background $duration-base;

		&:hover {
			background: $surface-3;
		}

		&:focus-visible {
			outline: 2px solid $accent-color;
			outline-offset: 2px;
		}
	}

	.tempo-display {
		display: flex;
		align-items: baseline;
		gap: $space-3;
		font-family: $font-mono;
		line-height: 1;
	}

	.tempo-bpm {
		font-size: 16px;
		font-weight: 500;
		color: $text-color;
	}

	.tempo-signature {
		font-size: 12px;
		color: $text-secondary;
	}

	.loop-range {
		display: flex;
		align-items: center;
		gap: $space-5;

		&--hidden {
			visibility: hidden;
		}
	}
</style>
