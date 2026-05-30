<script lang="ts">
	import { transport } from '../stores/transport';
	import { tempo } from '../stores/tempo';
	import { updateTempo, play, pause, stop, seek } from '../frontend/commands/dispatcher';
	import { exportTrigger, exportMp3Trigger } from '../stores/exportBus';
	import { settingsPanelOpen } from '../stores/preferences';
	import { leftPanel } from '../stores/leftPanel';
	import { rightPanel } from '../stores/rightPanel';
	import { viewMode } from '../stores/viewMode';
	import NumberInput from './NumberInput.svelte';

	import Icon from './Icon.svelte';

	let isRecording = false;

	function handleExport() {
		exportTrigger.update((n) => n + 1);
	}

	function handleExportMp3() {
		exportMp3Trigger.update((n) => n + 1);
	}

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

	function toggleRecord() {
		isRecording = !isRecording;
	}

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

	function handleBpm(v: number) {
		tempo.update((t) => ({ ...t, bpm: v }));
		updateTempo(v, $tempo.numerator, $tempo.denominator).catch(console.error);
	}

	function handleNumerator(v: number) {
		tempo.update((t) => ({ ...t, numerator: v }));
		updateTempo($tempo.bpm, v, $tempo.denominator).catch(console.error);
	}

	const VALID_DENOMINATORS = [2, 4, 8, 16];

	function cycleDenominator() {
		const idx = VALID_DENOMINATORS.indexOf($tempo.denominator);
		const next = VALID_DENOMINATORS[(idx + 1) % VALID_DENOMINATORS.length];
		tempo.update((t) => ({ ...t, denominator: next }));
		updateTempo($tempo.bpm, $tempo.numerator, next).catch(console.error);
	}
</script>

<div class="menu-bar">
	<div class="transport">
		<button class="btn" on:click={rewindToStart} title="Revenir au début">
			<!-- Skip to start -->
			<svg width="13" height="13" viewBox="0 0 13 13" fill="none" xmlns="http://www.w3.org/2000/svg">
				<rect x="1" y="1.5" width="1.6" height="10" rx="0.8" fill="currentColor"/>
				<path d="M11.5 2L4.5 6.5L11.5 11V2Z" fill="currentColor"/>
			</svg>
		</button>
		<button class="btn btn--play" on:click={togglePlayPause} title={$transport.isPlaying ? 'Pause' : 'Lecture'}>
			{#if $transport.isPlaying}
				<!-- Pause -->
				<svg width="11" height="13" viewBox="0 0 11 13" fill="none" xmlns="http://www.w3.org/2000/svg">
					<rect x="1" y="1" width="3" height="11" rx="1" fill="currentColor"/>
					<rect x="7" y="1" width="3" height="11" rx="1" fill="currentColor"/>
				</svg>
			{:else}
				<!-- Play -->
				<svg width="12" height="13" viewBox="0 0 12 13" fill="none" xmlns="http://www.w3.org/2000/svg">
					<path d="M2 1.5L11 6.5L2 11.5V1.5Z" fill="currentColor"/>
				</svg>
			{/if}
		</button>
		<button
			class="btn btn--record"
			class:active={isRecording}
			on:click={toggleRecord}
			title="Enregistrer"
		>
			<!-- Record dot -->
			<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg">
				<circle cx="6" cy="6" r="5" fill="currentColor"/>
			</svg>
		</button>
	</div>

	<div class="sep"></div>

	<div class="view-tabs">
		<button
			type="button"
			class="view-tab"
			class:active={$viewMode.main === 'arrange'}
			on:click={() => viewMode.setMain('arrange')}
			title="Arrangement"
		>
			Arrangement
		</button>
		<button
			type="button"
			class="view-tab"
			class:active={$viewMode.main === 'graph'}
			on:click={() => viewMode.setMain('graph')}
			title="Graphe audio (G)"
		>
			Graph
		</button>
	</div>

	<div class="sep"></div>

	<div class="position">
		<span class="ctrl-label">POSITION</span>
		<div class="position-display">
			<span class="pos-bar">{barStr}</span>
			<span class="pos-sep">.</span>
			<span class="pos-beat">{beatStr}</span>
		</div>
	</div>

	<div class="sep"></div>

	<div class="ctrl">
		<span class="ctrl-label">TEMPO</span>
		<div class="ctrl-row">
			<NumberInput
				value={$tempo.bpm}
				min={20}
				max={400}
				step={1}
				width="48px"
				unit="BPM"
				size="sm"
				on:change={(e) => handleBpm(e.detail)}
			/>
		</div>
	</div>

	<div class="ctrl">
		<span class="ctrl-label">MESURE</span>
		<div class="ctrl-row">
			<NumberInput
				value={$tempo.numerator}
				min={1}
				max={16}
				step={1}
				width="30px"
				size="sm"
				on:change={(e) => handleNumerator(e.detail)}
			/>
			<span class="ctrl-slash">/</span>
			<button class="denom-btn" on:click={cycleDenominator} title="Changer le dénominateur">{$tempo.denominator}</button>
		</div>
	</div>

	<div class="right-actions">
		<button
			class="btn btn--panel-toggle"
			class:active={$leftPanel.open}
			on:click={() => leftPanel.toggle()}
			title="Inspecteur (I)"
		>I</button>
		<button
			class="btn btn--panel-toggle"
			class:active={$rightPanel.open}
			on:click={() => rightPanel.toggle()}
			title="Bibliothèque (L)"
		>L</button>
		<div class="sep"></div>
		<button class="btn btn--export" on:click={handleExport} title="Exporter mixdown WAV… (Ctrl+Shift+E)">
			<svg width="10" height="11" viewBox="0 0 10 11" fill="none" xmlns="http://www.w3.org/2000/svg">
				<path d="M5 1V7M5 7L2.5 4.5M5 7L7.5 4.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>
				<path d="M1 9H9" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
			</svg>
			WAV
		</button>
		<button class="btn btn--export btn--export-mp3" on:click={handleExportMp3} title="Exporter mixdown MP3… (Ctrl+Shift+M)">
			<svg width="10" height="11" viewBox="0 0 10 11" fill="none" xmlns="http://www.w3.org/2000/svg">
				<path d="M5 1V7M5 7L2.5 4.5M5 7L7.5 4.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>
				<path d="M1 9H9" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
			</svg>
			MP3
		</button>
		<div class="sep"></div>
		<button class="btn btn--settings" on:click={() => settingsPanelOpen.set(true)} title="Préférences (Ctrl+,)">
			<Icon name="settings" size={14} />
		</button>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.menu-bar {
		background-color: $panel-background-color;
		height: 48px;
		display: flex;
		align-items: center;
		padding: 0 12px;
		gap: 12px;
		border-bottom: 1px solid $border-color;
		flex-shrink: 0;
	}

	// ── View tabs ─────────────────────────────────────────────────────────────
	.view-tabs {
		display: flex;
		gap: 2px;
		background: rgba(0, 0, 0, 0.25);
		border-radius: $radius-md;
		padding: 2px;
	}

	.view-tab {
		padding: 4px 10px;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: $text-muted;
		font-size: 11px;
		font-family: inherit;
		cursor: pointer;
		transition: background 0.1s, color 0.1s;

		&.active {
			background: rgba(21, 113, 255, 0.25);
			color: $text-color;
		}

		&:hover:not(.active) {
			color: $text-color;
		}
	}

	// ── Transport ─────────────────────────────────────────────────────────────
	.transport {
		display: flex;
		align-items: center;
		gap: 2px;
	}

	.btn {
		background: none;
		border: none;
		color: rgba(247, 247, 249, 0.65);
		cursor: pointer;
		width: 30px;
		height: 30px;
		border-radius: 4px;
		font-size: 13px;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: background 0.1s, color 0.1s;

		&:hover {
			background: rgba(255, 255, 255, 0.08);
			color: $text-color;
		}

		&--record {
			&.active {
				color: $color-error;
			}
			&:hover {
				color: #ff6b5f;
			}
		}

		&--export {
			font-size: 11px;
			width: auto;
			padding: 0 9px;
			letter-spacing: 0.4px;
			gap: 5px;
			display: flex;
			align-items: center;

			&-mp3 {
				color: #ff9a3c;
				&:hover {
					color: #ffb46a;
					background: rgba(255, 154, 60, 0.1);
				}
			}
		}

		&--settings {
			font-size: 15px;
			opacity: 0.6;
			transition: opacity 0.15s, background 0.1s;

			&:hover { opacity: 1; }
		}
	}

	.right-actions {
		margin-left: auto;
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.btn--panel-toggle {
		width: 26px;
		height: 26px;
		font-size: 11px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.05em;
		border-radius: $radius-sm;
		border: 1px solid transparent;
		transition: background 0.12s, color 0.12s, border-color 0.12s;

		&.active {
			background: rgba(21, 113, 255, 0.15);
			border-color: rgba(21, 113, 255, 0.35);
			color: $accent-color;
		}

		&:not(.active):hover {
			background: $surface-hover;
			color: $text-color;
		}
	}

	// ── Separator ─────────────────────────────────────────────────────────────
	.sep {
		width: 1px;
		height: 22px;
		background: $border-color;
		flex-shrink: 0;
	}

	// ── Position display ──────────────────────────────────────────────────────
	.position {
		display: flex;
		flex-direction: column;
		gap: 1px;
	}

	.position-display {
		display: flex;
		align-items: baseline;
		font-family: $font-mono;
		line-height: 1;
	}

	.pos-bar {
		font-size: 20px;
		font-weight: 500;
		color: $text-color;
		letter-spacing: 1px;
	}

	.pos-sep {
		font-size: 16px;
		color: rgba(247, 247, 249, 0.3);
		margin: 0 1px;
	}

	.pos-beat {
		font-size: 17px;
		color: rgba(247, 247, 249, 0.65);
	}

	// ── Generic control block ─────────────────────────────────────────────────
	.ctrl {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.ctrl-label {
		font-size: 8px;
		color: rgba(247, 247, 249, 0.3);
		text-transform: uppercase;
		letter-spacing: 0.9px;
		font-family: sans-serif;
		line-height: 1;
	}

	.ctrl-row {
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.ctrl-slash {
		font-size: 15px;
		color: rgba(247, 247, 249, 0.3);
		font-family: monospace;
		line-height: 1;
	}

	.denom-btn {
		width: 30px;
		height: 26px;
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: 3px;
		color: $text-color;
		font-family: 'SF Mono', 'Fira Code', ui-monospace, monospace;
		font-size: 14px;
		text-align: center;
		cursor: pointer;
		padding: 0;
		box-sizing: border-box;
		line-height: 1;
		transition: border-color 0.12s, background 0.12s;

		&:hover, &:focus-visible {
			border-color: $accent-color;
			background: rgba(21, 113, 255, 0.08);
			outline: none;
		}

		&:active {
			background: rgba(21, 113, 255, 0.14);
			border-color: $accent-color;
		}
	}

</style>
