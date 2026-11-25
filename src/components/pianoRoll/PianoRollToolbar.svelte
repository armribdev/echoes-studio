<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';
	import { MIDI_GRID_OPTIONS } from '../../lib/midiGrid';
	import type { PianoRollMode } from '../../stores/pianoRoll';

	export let editMode: boolean;
	export let mode: PianoRollMode;
	export let quantize: number;
	export let trackColor: string;

	const dispatch = createEventDispatcher<{
		setMode: PianoRollMode;
		quantizeChange: number;
		close: void;
		edit: void;
	}>();
</script>

{#if editMode}
	<div class="chrome" style="--track-color: {trackColor}">
		<div class="chrome-inner">
			<div class="mode-group" role="group" aria-label={$t('pianoRoll.editMode')}>
				<button
					class="mode-btn"
					class:active={mode === 'draw'}
					on:click={() => dispatch('setMode', 'draw')}
					title={$t('pianoRoll.drawTitle')}
					aria-label={$t('pianoRoll.draw')}
				>
					<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor" aria-hidden="true">
						<path d="M11.5 1.5a2.121 2.121 0 0 1 3 3L5 14H2v-3L11.5 1.5z" />
					</svg>
				</button>
				<button
					class="mode-btn"
					class:active={mode === 'select'}
					on:click={() => dispatch('setMode', 'select')}
					title={$t('pianoRoll.selectTitle')}
					aria-label={$t('pianoRoll.select')}
				>
					<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor" aria-hidden="true">
						<path d="M3 2l10 6-5 1.5L6.5 14 3 2z" />
					</svg>
				</button>
				<button
					class="mode-btn"
					class:active={mode === 'erase'}
					on:click={() => dispatch('setMode', 'erase')}
					title={$t('pianoRoll.eraseTitle')}
					aria-label={$t('pianoRoll.erase')}
				>
					<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor" aria-hidden="true">
						<path d="M9.5 2.5L14 7l-7 7H3v-3l6.5-8.5zM2 13h5.5l1-1.5H2V13z" />
					</svg>
				</button>
			</div>

			<div class="chrome-divider"></div>

			<select
				class="q-select"
				value={quantize}
				on:change={(e) => dispatch('quantizeChange', +e.currentTarget.value)}
				title={$t('pianoRoll.quantizeTitle')}
				aria-label={$t('pianoRoll.quantize')}
			>
				{#each MIDI_GRID_OPTIONS as opt}
					<option value={opt.beats}>{opt.label}</option>
				{/each}
			</select>

			<div class="chrome-spacer"></div>

			<button
				class="chrome-close"
				on:click={() => dispatch('close')}
				title={$t('common.closeTab')}
				aria-label={$t('common.close')}
			>
				<svg viewBox="0 0 10 10" width="10" height="10" fill="none" aria-hidden="true">
					<path
						d="M1.5 1.5L8.5 8.5M8.5 1.5L1.5 8.5"
						stroke="currentColor"
						stroke-width="1.5"
						stroke-linecap="round"
					/>
				</svg>
			</button>
		</div>
	</div>
{:else}
	<div class="chrome chrome--preview" style="--track-color: {trackColor}">
		<div class="chrome-inner">
			<div class="chrome-spacer"></div>
			<button class="edit-btn" on:click={() => dispatch('edit')}> Éditer </button>
			<button
				class="chrome-close"
				on:click={() => dispatch('close')}
				title={$t('common.closeTab')}
				aria-label={$t('common.close')}
			>
				<svg viewBox="0 0 10 10" width="10" height="10" fill="none" aria-hidden="true">
					<path
						d="M1.5 1.5L8.5 8.5M8.5 1.5L1.5 8.5"
						stroke="currentColor"
						stroke-width="1.5"
						stroke-linecap="round"
					/>
				</svg>
			</button>
		</div>
	</div>
{/if}

<style lang="scss">
	.chrome {
		display: flex;
		flex-shrink: 0;
		height: 30px;
		background: #1d1d27;
		border-bottom: 1px solid #2e2e3c;
		box-shadow: inset 3px 0 0 var(--track-color, #4d96ff);

		&--preview {
			background: #1a1a22;
			border-bottom-color: #282832;
		}
	}

	.chrome-inner {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 0 8px 0 11px;
		overflow-x: auto;
		overflow-y: hidden;
		scrollbar-width: none;
		&::-webkit-scrollbar {
			display: none;
		}
	}

	.chrome-divider {
		width: 1px;
		height: 16px;
		background: rgba(255, 255, 255, 0.07);
		flex-shrink: 0;
	}

	.chrome-spacer {
		flex: 1;
		min-width: 8px;
	}

	.chrome-close {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 22px;
		height: 22px;
		padding: 0;
		background: transparent;
		border: none;
		border-radius: 4px;
		color: #666678;
		cursor: pointer;
		flex-shrink: 0;
		transition:
			background 0.12s,
			color 0.12s;

		&:hover {
			background: rgba(255, 65, 54, 0.14);
			color: #ff6b60;
		}
	}

	.edit-btn {
		height: 22px;
		padding: 0 10px;
		background: rgba(21, 113, 255, 0.12);
		border: 1px solid rgba(21, 113, 255, 0.28);
		border-radius: 4px;
		color: #6aadff;
		font-size: 11px;
		font-weight: 500;
		cursor: pointer;
		flex-shrink: 0;
		transition:
			background 0.12s,
			border-color 0.12s;

		&:hover {
			background: rgba(21, 113, 255, 0.2);
			border-color: rgba(21, 113, 255, 0.45);
		}
	}

	.q-select {
		height: 22px;
		padding: 0 6px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.08);
		border-radius: 4px;
		color: #c8c8d8;
		font-size: 11px;
		cursor: pointer;
		flex-shrink: 0;
		transition:
			border-color 0.12s,
			background 0.12s;

		&:hover {
			background: rgba(255, 255, 255, 0.07);
			border-color: rgba(255, 255, 255, 0.14);
		}
		&:focus-visible {
			outline: none;
			border-color: #1571ff;
		}
	}

	.mode-group {
		display: flex;
		gap: 1px;
		flex-shrink: 0;
		background: rgba(0, 0, 0, 0.25);
		border-radius: 6px;
		padding: 2px;
	}

	.mode-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 26px;
		height: 22px;
		padding: 0;
		background: transparent;
		border: none;
		border-radius: 4px;
		color: #666678;
		cursor: pointer;
		flex-shrink: 0;
		transition:
			background 0.12s,
			color 0.12s;

		&:hover {
			background: rgba(255, 255, 255, 0.07);
			color: #d0d0e0;
		}

		&.active {
			background: rgba(21, 113, 255, 0.2);
			color: #6aadff;
		}
	}
</style>
