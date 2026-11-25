<!-- SamplerPads.svelte — 16 drum pads of the built-in sampler (ADR-028) -->
<script lang="ts">
	import { open as openDialog } from '@tauri-apps/plugin-dialog';
	import ParamKnob from './ParamKnob.svelte';
	import CustomSelect from './CustomSelect.svelte';
	import type { PadPlayMode, SamplerKit } from '../types/global';
	import type { SamplerPadPatch } from '../shared/protocol/commands';
	import { setSamplerPad } from '../frontend/commands/dispatcher';
	import { auditionPitch, releaseAudition } from '../frontend/pianoRoll/noteAudition';
	import { AUDIO_EXTENSIONS } from '../frontend/library/types';
	import { padLabel, padRows } from '../frontend/sampler/pads';
	import { notify } from '../stores/status';

	export let trackId: string;
	export let kit: SamplerKit;

	const AUDITION_VELOCITY = 100;
	const MODE_OPTIONS = [
		{ value: 'one_shot', label: 'One-shot' },
		{ value: 'gate', label: 'Gate' }
	];
	const CHOKE_OPTIONS = [
		{ value: 0, label: 'Aucune coupure' },
		...[1, 2, 3, 4].map((group) => ({ value: group, label: `Coupure ${group}` }))
	];

	let selected = 0;

	$: rows = padRows(kit.pads.length);
	$: pad = kit.pads[selected] ?? kit.pads[0];

	async function update(patch: SamplerPadPatch) {
		try {
			await setSamplerPad(trackId, selected, patch);
		} catch (e) {
			notify(`Erreur sampler : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	function setMode(mode: string | number) {
		update({ mode: mode as PadPlayMode });
	}

	function press(index: number) {
		selected = index;
		auditionPitch(trackId, kit.pads[index].note, AUDITION_VELOCITY);
	}

	async function chooseFile() {
		try {
			const result = await openDialog({
				multiple: false,
				filters: [{ name: 'Audio', extensions: [...AUDIO_EXTENSIONS] }]
			});
			if (typeof result === 'string') await update({ sample_path: result });
		} catch (e) {
			notify(`Erreur ouverture fichier : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	const formatDb = (v: number) => `${v.toFixed(1)} dB`;
	const formatPan = (v: number) =>
		Math.abs(v) < 0.005 ? 'C' : `${v < 0 ? 'L' : 'R'}${Math.round(Math.abs(v) * 100)}`;
	const formatTune = (v: number) => `${v > 0 ? '+' : ''}${v.toFixed(2)} st`;
	const formatMs = (v: number) =>
		v >= 1000 ? `${(v / 1000).toFixed(2)} s` : `${Math.round(v)} ms`;
</script>

<div class="pads">
	{#each rows as row}
		<div class="pad-row">
			{#each row as index}
				{@const cell = kit.pads[index]}
				<button
					class="pad"
					class:selected={index === selected}
					class:loaded={!!cell.sample_path && !cell.missing}
					class:missing={cell.missing}
					type="button"
					data-sampler-pad={index}
					data-track-id={trackId}
					title={cell.missing
						? `Fichier introuvable : ${cell.sample_path}`
						: (cell.sample_path ?? '')}
					on:pointerdown={() => press(index)}
					on:pointerup={releaseAudition}
					on:pointerleave={releaseAudition}
				>
					{padLabel(cell)}
				</button>
			{/each}
		</div>
	{/each}
</div>

{#if pad}
	<div class="pad-file">
		<button class="file-btn" type="button" on:click={chooseFile}>Choisir…</button>
		{#if pad.sample_path}
			<button class="rm-btn" type="button" on:click={() => update({ sample_path: null })}>×</button>
		{/if}
	</div>

	<div class="pad-knobs">
		<ParamKnob
			label="Gain"
			value={pad.gain_db}
			min={-60}
			max={12}
			defaultValue={0}
			format={formatDb}
			on:release={(e) => update({ gain_db: e.detail })}
		/>
		<ParamKnob
			label="Pan"
			value={pad.pan}
			min={-1}
			max={1}
			defaultValue={0}
			format={formatPan}
			on:release={(e) => update({ pan: e.detail })}
		/>
		<ParamKnob
			label="Tune"
			value={pad.tune_semitones}
			min={-24}
			max={24}
			defaultValue={0}
			format={formatTune}
			on:release={(e) => update({ tune_semitones: e.detail })}
		/>
		<ParamKnob
			label="Att"
			value={pad.attack_ms}
			min={0}
			max={100}
			defaultValue={0}
			format={formatMs}
			on:release={(e) => update({ attack_ms: e.detail })}
		/>
		{#if pad.mode === 'gate'}
			<ParamKnob
				label="Rel"
				value={pad.release_ms}
				min={5}
				max={2000}
				defaultValue={10}
				scale="log"
				format={formatMs}
				on:release={(e) => update({ release_ms: e.detail })}
			/>
		{/if}
	</div>

	<div class="pad-selects">
		<CustomSelect
			small
			value={pad.mode}
			options={MODE_OPTIONS}
			on:change={(e) => setMode(e.detail)}
		/>
		<CustomSelect
			small
			value={pad.choke_group}
			options={CHOKE_OPTIONS}
			on:change={(e) => update({ choke_group: Number(e.detail) })}
		/>
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.pads {
		display: flex;
		flex-direction: column;
		gap: 3px;
		padding: 6px;
	}

	.pad-row {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 3px;
	}

	.pad {
		height: 30px;
		min-width: 0;
		padding: 0 3px;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		background: $panel-background-color;
		color: $text-subtle;
		font-size: 9px;
		font-family: $font-mono;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		cursor: pointer;

		&.loaded {
			color: $text-color;
			background: $surface-hover;
		}

		&.missing {
			color: $text-faint;
			text-decoration: line-through;
		}

		&.selected {
			border-color: $accent-color;
		}

		&:hover {
			background: $surface-hover;
		}

		&:focus-visible {
			outline: 1px solid $accent-hover;
			outline-offset: 1px;
		}
	}

	.pad-file {
		display: flex;
		gap: 4px;
		padding: 0 6px 4px;
	}

	.file-btn,
	.rm-btn {
		font-size: 10px;
		font-family: $font-mono;
		padding: 3px 7px;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		background: transparent;
		color: $text-subtle;
		cursor: pointer;

		&:hover {
			background: $accent-soft;
			color: $text-color;
		}
	}

	.file-btn {
		flex: 1;
	}

	.pad-knobs {
		display: flex;
		align-items: flex-end;
		gap: 4px;
		padding: 2px 6px 4px;
	}

	.pad-selects {
		display: flex;
		gap: 4px;
		padding: 0 6px 6px;

		:global(> *) {
			flex: 1;
			min-width: 0;
		}
	}
</style>
