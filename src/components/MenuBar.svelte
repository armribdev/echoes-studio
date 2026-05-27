<script lang="ts">
	import { transport } from '../stores/transport';
	import { tempo } from '../stores/tempo';
	import { updateTempo, play, pause, stop, seek } from '../frontend/commands/dispatcher';

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

	function handleBpm(e: Event) {
		const v = parseFloat((e.target as HTMLInputElement).value);
		if (v >= 20 && v <= 400) {
			tempo.update((t) => ({ ...t, bpm: v }));
			updateTempo(v, $tempo.numerator, $tempo.denominator).catch(console.error);
		} else {
			(e.target as HTMLInputElement).value = String($tempo.bpm);
		}
	}

	function handleNumerator(e: Event) {
		const v = parseInt((e.target as HTMLInputElement).value);
		if (v >= 1 && v <= 16) {
			tempo.update((t) => ({ ...t, numerator: v }));
			updateTempo($tempo.bpm, v, $tempo.denominator).catch(console.error);
		} else {
			(e.target as HTMLInputElement).value = String($tempo.numerator);
		}
	}

	function handleDenominator(e: Event) {
		const v = parseInt((e.target as HTMLSelectElement).value);
		tempo.update((t) => ({ ...t, denominator: v }));
		updateTempo($tempo.bpm, $tempo.numerator, v).catch(console.error);
	}
</script>

<div class="menu-bar">
	<div class="transport">
		<button class="btn" on:click={rewindToStart} title="Revenir au début">⏮</button>
		<button class="btn btn--play" on:click={togglePlayPause} title={$transport.isPlaying ? 'Pause' : 'Lecture'}>
			{$transport.isPlaying ? '⏸' : '▶'}
		</button>
		<button
			class="btn btn--record"
			class:active={isRecording}
			on:click={toggleRecord}
			title="Enregistrer"
		>⏺</button>
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
			<input
				class="num-input"
				style="width:48px"
				type="number"
				min="20"
				max="400"
				step="1"
				value={$tempo.bpm}
				on:change={handleBpm}
			/>
			<span class="ctrl-unit">BPM</span>
		</div>
	</div>

	<div class="ctrl">
		<span class="ctrl-label">MESURE</span>
		<div class="ctrl-row">
			<input
				class="num-input"
				style="width:26px"
				type="number"
				min="1"
				max="16"
				step="1"
				value={$tempo.numerator}
				on:change={handleNumerator}
			/>
			<span class="ctrl-slash">/</span>
			<select class="num-input sel-input" value={$tempo.denominator} on:change={handleDenominator}>
				<option value={2}>2</option>
				<option value={4}>4</option>
				<option value={8}>8</option>
				<option value={16}>16</option>
			</select>
		</div>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.menu-bar {
		background-color: $pannel-background-color;
		height: 48px;
		display: flex;
		align-items: center;
		padding: 0 12px;
		gap: 12px;
		border-bottom: 1px solid $border-color;
		flex-shrink: 0;
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
				color: #ff4136;
			}
			&:hover {
				color: #ff7366;
			}
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
		font-family: 'SF Mono', 'Fira Code', ui-monospace, monospace;
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

	.ctrl-unit {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.35);
		font-family: sans-serif;
	}

	.ctrl-slash {
		font-size: 15px;
		color: rgba(247, 247, 249, 0.3);
		font-family: monospace;
		line-height: 1;
	}

	// ── Number inputs & select ────────────────────────────────────────────────
	.num-input {
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: 3px;
		color: $text-color;
		font-family: 'SF Mono', 'Fira Code', ui-monospace, monospace;
		font-size: 14px;
		padding: 2px 4px;
		text-align: center;
		outline: none;
		-moz-appearance: textfield;
		line-height: 1;

		&::-webkit-inner-spin-button,
		&::-webkit-outer-spin-button {
			-webkit-appearance: none;
		}

		&:focus {
			border-color: $accent-color;
			background: rgba(21, 113, 255, 0.08);
		}
	}

	.sel-input {
		appearance: none;
		-webkit-appearance: none;
		cursor: pointer;
		width: 32px;
	}
</style>
