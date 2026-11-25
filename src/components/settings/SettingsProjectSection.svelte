<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '../Button.svelte';
	import Field from '../Field.svelte';
	import FieldGrid from '../FieldGrid.svelte';
	import NumberInput from '../NumberInput.svelte';
	import SegmentedControl from '../SegmentedControl.svelte';
	import Section from '../Section.svelte';
	import SubSection from '../SubSection.svelte';
	import TextInput from '../TextInput.svelte';
	import TimeSignatureInput from '../TimeSignatureInput.svelte';
	import Toggle from '../Toggle.svelte';
	import type { Preferences } from '../../stores/preferences';

	export let project: Preferences['project'];
</script>

<Section title={$t('settings.project.title')}>
	<Field label={$t('settings.project.defaultFolder')}>
		<div class="field-row">
			<TextInput bind:value={project.defaultFolder} placeholder="~/Documents/Echoes Studio" />
			<Button compact on:click={() => {}}>{$t('common.browse')}</Button>
		</div>
	</Field>

	<SubSection title={$t('settings.project.autoSaveTitle')}>
		<Toggle
			bind:checked={project.autoSave}
			label={$t('settings.project.autoSave')}
			description={$t('settings.project.autoSaveDesc')}
		/>

		{#if project.autoSave}
			<FieldGrid>
				<Field label={$t('settings.project.interval')}>
					<NumberInput bind:value={project.autoSaveIntervalSec} min={10} max={600} unit="sec" />
				</Field>
				<Field label={$t('settings.project.backupsKept')}>
					<NumberInput bind:value={project.autoSaveMaxBackups} min={1} max={20} unit="max" />
				</Field>
			</FieldGrid>
		{/if}
	</SubSection>
	<SubSection title={$t('settings.project.exportTitle')}>
		<Field label={$t('settings.project.wavBitDepth')}>
			<SegmentedControl
				bind:value={project.exportBitDepth}
				options={[
					{ value: 16, label: '16 bit' },
					{ value: 24, label: '24 bit' },
					{ value: 32, label: '32-bit float' }
				]}
			/>
			<span slot="hint">
				{#if project.exportBitDepth === 16}{$t('settings.project.wav16')}
				{:else if project.exportBitDepth === 24}{$t('settings.project.wav24')}
				{:else}{$t('settings.project.wav32')}{/if}
			</span>
		</Field>

		<Field label={$t('settings.project.mp3Bitrate')}>
			<SegmentedControl
				bind:value={project.exportMp3Bitrate}
				options={[
					{ value: 128, label: '128 kbps' },
					{ value: 192, label: '192 kbps' },
					{ value: 320, label: '320 kbps' }
				]}
			/>
			<span slot="hint">
				{#if project.exportMp3Bitrate === 128}{$t('settings.project.mp3_128')}
				{:else if project.exportMp3Bitrate === 192}{$t('settings.project.mp3_192')}
				{:else}{$t('settings.project.mp3_320')}{/if}
			</span>
		</Field>
	</SubSection>
	<SubSection title={$t('settings.project.newProjectDefaults')}>
		<FieldGrid>
			<Field label={$t('settings.project.bpm')}>
				<NumberInput bind:value={project.defaultBpm} min={20} max={400} unit="BPM" />
			</Field>
			<Field label={$t('settings.project.timeSignature')}>
				<TimeSignatureInput
					bind:numerator={project.defaultNumerator}
					bind:denominator={project.defaultDenominator}
				/>
			</Field>
		</FieldGrid>
	</SubSection>
</Section>

<style lang="scss">
	@use '../../variables.scss' as *;

	.field-row {
		display: flex;
		align-items: center;
		gap: 7px;
	}
</style>
