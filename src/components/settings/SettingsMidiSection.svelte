<script context="module" lang="ts">
	import type { KnownMidiDevice } from '../../stores/preferences';

	export interface MidiDeviceRow extends KnownMidiDevice {
		connected: boolean;
	}
</script>

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Section from '../Section.svelte';
	import SubSection from '../SubSection.svelte';
	import Toggle from '../Toggle.svelte';
	import type { Preferences } from '../../stores/preferences';

	export let rows: MidiDeviceRow[];
	export let midi: Preferences['midi'];

	let renamingMidiDevice: string | null = null;

	function midiDeviceHint(row: MidiDeviceRow): string {
		return row.alias ? row.name : '';
	}

	function startMidiRename(event: MouseEvent, name: string) {
		if ((event.target as HTMLElement).closest('button')) return;
		renamingMidiDevice = name;
	}

	// Enter and blur both land here; the guard ignores the blur that follows Enter.
	function finishMidiRename(row: MidiDeviceRow, value: string) {
		if (renamingMidiDevice !== row.name) return;
		const name = value.trim();
		row.alias = name === row.name ? '' : name;
		rows = rows;
		renamingMidiDevice = null;
	}

	function focusAndSelect(node: HTMLInputElement) {
		node.focus();
		node.select();
	}
</script>

<Section title={$t('settings.midi.title')}>
	<span class="field-hint">{$t('settings.midi.intro')}</span>

	<SubSection>
		{#if rows.length === 0}
			<span class="field-hint">{$t('settings.midi.none')}</span>
		{:else}
			{#each rows as row (row.name)}
				<!-- svelte-ignore a11y-no-static-element-interactions -->
				<div
					class:midi-device--offline={!row.connected}
					title={$t('settings.midi.renameHint')}
					on:dblclick={(e) => startMidiRename(e, row.name)}
				>
					{#if renamingMidiDevice === row.name}
						<Toggle bind:checked={row.enabled} label={row.alias || row.name}>
							<input
								slot="info"
								class="midi-rename"
								type="text"
								value={row.alias || row.name}
								aria-label={$t('settings.midi.renameAria', {
									values: { name: row.name }
								})}
								use:focusAndSelect
								on:keydown={(e) => {
									if (e.key === 'Enter') finishMidiRename(row, e.currentTarget.value);
								}}
								on:blur={(e) => finishMidiRename(row, e.currentTarget.value)}
							/>
						</Toggle>
					{:else}
						<Toggle
							bind:checked={row.enabled}
							label={row.alias || row.name}
							description={midiDeviceHint(row)}
						/>
					{/if}
				</div>
			{/each}
		{/if}
	</SubSection>
	<SubSection title={$t('settings.midi.keyboardSound')}>
		<Toggle
			bind:checked={midi.localControlOff}
			label={$t('settings.midi.localControlOff')}
			description={$t('settings.midi.localControlOffDesc')}
		/>
	</SubSection>
</Section>

<style lang="scss">
	@use '../../variables.scss' as *;

	.field-hint {
		line-height: 1.2;
		font-size: 10px;
		color: $text-faint;
	}

	.midi-device--offline {
		opacity: 0.45;
	}

	// Sits where the Toggle label is, so the row keeps its height while renaming.
	.midi-rename {
		width: 100%;
		padding: 0;
		border: none;
		background: transparent;
		color: $text-color;
		font: inherit;
		font-size: 12px;
		font-weight: 500;
		outline: none;
	}
</style>
