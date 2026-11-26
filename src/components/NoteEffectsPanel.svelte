<!-- NoteEffectsPanel.svelte — transpose, scale and chord ahead of the arpeggiator (ADR-038) -->
<script lang="ts">
	import CustomSelect from './CustomSelect.svelte';
	import NumberInput from './NumberInput.svelte';
	import type { ChordShape, NoteEffectSettings, ScaleKind } from '../types/global';
	import type { NoteEffectsPatch } from '../shared/protocol/commands';
	import {
		CHORD_SHAPE_OPTIONS,
		DEFAULT_NOTE_EFFECTS,
		MAX_TRANSPOSE_SEMITONES,
		MIN_TRANSPOSE_SEMITONES,
		SCALE_OPTIONS,
		SCALE_ROOT_OPTIONS
	} from '../shared/project/noteEffects';
	import { setNoteEffects } from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';

	export let trackId: string;
	export let noteEffects: NoteEffectSettings | null = null;

	$: settings = noteEffects ?? DEFAULT_NOTE_EFFECTS;

	async function update(patch: NoteEffectsPatch) {
		try {
			await setNoteEffects(trackId, patch);
		} catch (e) {
			notify(`Erreur effets de notes : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	function setScaleRoot(root: string | number) {
		update({ scale: { root: Number(root) } });
	}

	function setScale(scale: string | number) {
		update({ scale: { scale: scale as ScaleKind } });
	}

	function setChordShape(shape: string | number) {
		update({ chord: { shape: shape as ChordShape } });
	}
</script>

<div class="fx">
	<div class="fx-head">
		<span class="fx-title">Effets de notes</span>
	</div>

	<div class="row" class:off={!settings.transpose.enabled}>
		<div class="row-head">
			<button
				class="power"
				class:on={settings.transpose.enabled}
				type="button"
				aria-pressed={settings.transpose.enabled}
				title="Transposition"
				on:click={() => update({ transpose: { enabled: !settings.transpose.enabled } })}
			>
				<svg width="8" height="8" viewBox="0 0 8 8"
					><circle cx="4" cy="4" r="3" fill="currentColor" /></svg
				>
			</button>
			<span class="row-title">Transposition</span>
		</div>
		<div class="row-controls">
			<NumberInput
				size="sm"
				width="72px"
				unit="st"
				min={MIN_TRANSPOSE_SEMITONES}
				max={MAX_TRANSPOSE_SEMITONES}
				value={settings.transpose.semitones}
				on:change={(e) => update({ transpose: { semitones: Math.round(e.detail) } })}
			/>
		</div>
	</div>

	<div class="row" class:off={!settings.scale.enabled}>
		<div class="row-head">
			<button
				class="power"
				class:on={settings.scale.enabled}
				type="button"
				aria-pressed={settings.scale.enabled}
				title="Gamme"
				on:click={() => update({ scale: { enabled: !settings.scale.enabled } })}
			>
				<svg width="8" height="8" viewBox="0 0 8 8"
					><circle cx="4" cy="4" r="3" fill="currentColor" /></svg
				>
			</button>
			<span class="row-title">Gamme</span>
		</div>
		<div class="row-controls">
			<CustomSelect
				small
				size="sm"
				value={settings.scale.root}
				options={SCALE_ROOT_OPTIONS}
				on:change={(e) => setScaleRoot(e.detail)}
			/>
			<CustomSelect
				small
				size="sm"
				value={settings.scale.scale}
				options={SCALE_OPTIONS}
				on:change={(e) => setScale(e.detail)}
			/>
		</div>
	</div>

	<div class="row" class:off={!settings.chord.enabled}>
		<div class="row-head">
			<button
				class="power"
				class:on={settings.chord.enabled}
				type="button"
				aria-pressed={settings.chord.enabled}
				title="Accord"
				on:click={() => update({ chord: { enabled: !settings.chord.enabled } })}
			>
				<svg width="8" height="8" viewBox="0 0 8 8"
					><circle cx="4" cy="4" r="3" fill="currentColor" /></svg
				>
			</button>
			<span class="row-title">Accord</span>
		</div>
		<div class="row-controls">
			<CustomSelect
				small
				size="sm"
				value={settings.chord.shape}
				options={CHORD_SHAPE_OPTIONS}
				on:change={(e) => setChordShape(e.detail)}
			/>
		</div>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.fx {
		border-top: 1px solid $border-faint;
		border-bottom: 1px solid $border-faint;
	}

	.fx-head {
		padding: $space-2 $panel-inset;
		background: $panel-background-color;
	}

	.fx-title {
		font-size: 10px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.08em;
		color: $text-muted;
		text-transform: uppercase;
	}

	.row {
		padding: $space-2 $panel-inset $space-3;

		&.off .row-controls {
			opacity: 0.5;
		}
	}

	.row-head {
		display: flex;
		align-items: center;
		gap: $space-2;
	}

	.power {
		@include power-button;
	}

	.row-title {
		font-size: 11px;
		color: $text-muted;
	}

	.row-controls {
		display: flex;
		gap: $space-2;
		padding-top: $space-2;

		:global(> *) {
			flex: 1;
			min-width: 0;
		}
	}
</style>
