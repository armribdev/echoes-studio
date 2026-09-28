<script lang="ts">
	import { transport } from '../stores/transport';
	import { loopRegion } from '../stores/loopRegion';
	import { tempo } from '../stores/tempo';
	import { play, pause, stop, seek } from '../frontend/commands/dispatcher';
	import { leftPanel } from '../stores/leftPanel';
	import { rightPanel } from '../stores/rightPanel';
	import Icon from './Icon.svelte';

	let isRecording = false;

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

</script>

<div class="menu-bar">
	<div class="zone zone--left">
		<button
			class="btn btn--panel-toggle"
			class:active={$leftPanel.open}
			on:click={() => leftPanel.toggle()}
			title="Inspecteur (I)"
			aria-label="Inspecteur"
		>
			<Icon name="inspector" size={14} />
		</button>
	</div>

	<div class="zone zone--center">
		<div class="transport">
			<button class="btn" on:click={rewindToStart} title="Revenir au début">
				<svg width="13" height="13" viewBox="0 0 13 13" fill="none" xmlns="http://www.w3.org/2000/svg">
					<rect x="1" y="1.5" width="1.6" height="10" rx="0.8" fill="currentColor"/>
					<path d="M11.5 2L4.5 6.5L11.5 11V2Z" fill="currentColor"/>
				</svg>
			</button>
			<button
				class="btn btn--loop"
				class:active={$loopRegion.enabled}
				on:click={() => loopRegion.toggle()}
				title="Boucle de lecture"
				aria-pressed={$loopRegion.enabled}
			>
				<svg width="14" height="12" viewBox="0 0 14 12" fill="none" xmlns="http://www.w3.org/2000/svg">
					<path d="M3.2 2.2H9.2V0.4L12.6 3.2L9.2 6V4.2H3.2V2.2Z" fill="currentColor"/>
					<path d="M10.8 9.8H4.8V11.6L1.4 8.8L4.8 6V7.8H10.8V9.8Z" fill="currentColor"/>
				</svg>
			</button>
			<button class="btn btn--play" on:click={togglePlayPause} title={$transport.isPlaying ? 'Pause' : 'Lecture'}>
				{#if $transport.isPlaying}
					<svg width="11" height="13" viewBox="0 0 11 13" fill="none" xmlns="http://www.w3.org/2000/svg">
						<rect x="1" y="1" width="3" height="11" rx="1" fill="currentColor"/>
						<rect x="7" y="1" width="3" height="11" rx="1" fill="currentColor"/>
					</svg>
				{:else}
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
				<svg width="12" height="12" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg">
					<circle cx="6" cy="6" r="5" fill="currentColor"/>
				</svg>
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
	</div>

	<div class="zone zone--right">
		<button
			class="btn btn--panel-toggle"
			class:active={$rightPanel.open}
			on:click={() => rightPanel.toggle()}
			title="Bibliothèque (B)"
			aria-label="Bibliothèque"
		>
			<Icon name="library" size={14} />
		</button>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.menu-bar {
		background-color: $panel-background-color;
		height: 48px;
		display: grid;
		grid-template-columns: 1fr auto 1fr;
		align-items: center;
		padding: 0 12px;
		border-bottom: 1px solid $border-color;
		flex-shrink: 0;
	}

	.zone {
		display: flex;
		align-items: center;
		min-width: 0;

		&--left {
			justify-content: flex-start;
			gap: 4px;
		}

		&--center {
			justify-content: center;
			gap: 12px;
		}

		&--right {
			justify-content: flex-end;
			gap: 4px;
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

		&--loop.active {
			color: #4d94ff;
		}

		&--record {
			&.active {
				color: $color-error;
			}
			&:hover {
				color: #ff6b5f;
			}
		}

	}

	.btn--panel-toggle {
		width: 30px;
		height: 30px;
		border-radius: $radius-sm;
		border: 1px solid $border-subtle;
		background: $surface-2;
		transition: background 0.12s, color 0.12s, border-color 0.12s;

		&:not(.active):hover {
			background: $surface-3;
			border-color: rgba(255, 255, 255, 0.16);
			color: $text-color;
		}

		&.active {
			background: rgba(21, 113, 255, 0.15);
			border-color: rgba(21, 113, 255, 0.45);
			color: $accent-color;
		}

		&:focus-visible {
			outline: 2px solid $accent-color;
			outline-offset: 2px;
		}
	}

	// ── Separator ─────────────────────────────────────────────────────────────
	.sep {
		width: 1px;
		height: 22px;
		background: $border-color;
		flex-shrink: 0;
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

	.position {
		display: flex;
		flex-direction: column;
		gap: 2px;
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
		color: rgba(247, 247, 249, 0.3);
		margin: 0 1px;
	}

	.pos-beat {
		font-size: 21px;
		color: rgba(247, 247, 249, 0.65);
	}

</style>
