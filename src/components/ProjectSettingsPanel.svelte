<script lang="ts">
	import { get } from 'svelte/store';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { projectSettingsPanelOpen, projectPath, projectName } from '../stores/project';
	import { tempo } from '../stores/tempo';
	import { midiGridBeats } from '../stores/project';
	import { engineProject, engineProjectName } from '../frontend/store/engineStore';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import {
		currentProjectMusical,
		updateProjectMusical,
		updateProjectName
	} from '../frontend/commands/dispatcher';
	import { normalizeMidiGridBeats } from '../lib/midiGrid';
	import {
		DEFAULT_KEY_MODE,
		DEFAULT_KEY_ROOT,
		DEFAULT_MIDI_PPQ,
		DEFAULT_SWING_PERCENT,
		KEY_MODE_OPTIONS,
		KEY_ROOT_OPTIONS,
		MIDI_PPQ_OPTIONS,
		musicalFieldsFromDto,
		type ProjectMusicalFields
	} from '../lib/projectMusical';
	import CustomSelect from './CustomSelect.svelte';
	import NumberInput from './NumberInput.svelte';
	import PillGroup from './PillGroup.svelte';
	import Button from './Button.svelte';
	import Icon from './Icon.svelte';
	import Dialog from './Dialog.svelte';
	import DialogFooter from './DialogFooter.svelte';
	import DialogHeader from './DialogHeader.svelte';
	import Field from './Field.svelte';
	import FieldGrid from './FieldGrid.svelte';
	import Section from './Section.svelte';
	import SubSection from './SubSection.svelte';
	import TextInput from './TextInput.svelte';
	import TimeSignatureInput from './TimeSignatureInput.svelte';
	import { t } from 'svelte-i18n';

	let draftName = '';
	let draftMusical: ProjectMusicalFields = musicalFieldsFromDto({
		bpm: 120,
		numerator: 4,
		denominator: 4,
		midi_grid_beats: 0.25,
		key_root: DEFAULT_KEY_ROOT,
		key_mode: DEFAULT_KEY_MODE,
		swing_percent: DEFAULT_SWING_PERCENT,
		midi_ppq: DEFAULT_MIDI_PPQ,
		midi_grid_use_triplets: false
	});
	let applyError: string | null = null;

	function syncDraftFromStores() {
		draftName = get(engineProjectName);
		const p = get(engineProject);
		const t = get(tempo);
		draftMusical = musicalFieldsFromDto({
			bpm: t.bpm,
			numerator: t.numerator,
			denominator: t.denominator,
			midi_grid_beats: normalizeMidiGridBeats(get(midiGridBeats)),
			key_root: p.key_root,
			key_mode: p.key_mode,
			swing_percent: p.swing_percent,
			midi_ppq: p.midi_ppq,
			midi_grid_use_triplets: p.midi_grid_use_triplets
		});
	}

	$: if ($projectSettingsPanelOpen) {
		keyboardFocus.setSettings();
		syncDraftFromStores();
	}

	function cancelAndClose() {
		applyError = null;
		projectSettingsPanelOpen.set(false);
		keyboardFocus.setTimeline();
	}

	async function applyAndClose() {
		applyError = null;
		const trimmed = draftName.trim() || get(t)('common.untitled');
		const cur = currentProjectMusical();
		const nextMusical: ProjectMusicalFields = {
			...draftMusical,
			midi_grid_beats: normalizeMidiGridBeats(cur.midi_grid_beats)
		};

		try {
			if (trimmed !== get(engineProjectName)) {
				await updateProjectName(trimmed);
				projectName.set(trimmed);
				await getCurrentWindow().setTitle(`${trimmed} — Echoes Studio`);
			}
			const musicalChanged =
				nextMusical.bpm !== cur.bpm ||
				nextMusical.numerator !== cur.numerator ||
				nextMusical.denominator !== cur.denominator ||
				nextMusical.midi_grid_beats !== cur.midi_grid_beats ||
				nextMusical.key_root !== cur.key_root ||
				nextMusical.key_mode !== cur.key_mode ||
				nextMusical.swing_percent !== cur.swing_percent ||
				nextMusical.midi_ppq !== cur.midi_ppq ||
				nextMusical.midi_grid_use_triplets !== cur.midi_grid_use_triplets;

			if (musicalChanged) {
				tempo.set({
					bpm: nextMusical.bpm,
					numerator: nextMusical.numerator,
					denominator: nextMusical.denominator
				});
				await updateProjectMusical(nextMusical);
			}
			projectSettingsPanelOpen.set(false);
			keyboardFocus.setTimeline();
		} catch (err) {
			applyError =
				typeof err === 'string'
					? err
					: ((err as Error)?.message ?? get(t)('projectSettings.saveFailed'));
		}
	}

	function formatPath(path: string | null): string {
		if (!path) return get(t)('projectSettings.unsaved');
		const sep = path.includes('\\') ? '\\' : '/';
		const parts = path.split(sep);
		if (parts.length <= 2) return path;
		return `…${sep}${parts.slice(-2).join(sep)}`;
	}
</script>

{#if $projectSettingsPanelOpen}
	<Dialog label={$t('projectSettings.aria')} width={500} on:close={cancelAndClose}>
		<DialogHeader title={$t('projectSettings.title')} on:close={cancelAndClose}>
			<Icon slot="icon" name="folder" size={14} />
		</DialogHeader>

		<div class="panel-body">
			<Section title={$t('projectSettings.identity')}>
				<Field label={$t('projectSettings.projectName')}>
					<TextInput bind:value={draftName} placeholder={$t('common.untitled')} maxlength={120} />
				</Field>
				<Field label={$t('projectSettings.file')}>
					<span class="path-display" title={$projectPath ?? undefined}>
						{formatPath($projectPath)}
					</span>
				</Field>
			</Section>

			<SubSection title={$t('projectSettings.musical')}>
				<Field hint={$t('projectSettings.musicalHint')} />
				<FieldGrid>
					<Field label={$t('projectSettings.tempo')}>
						<NumberInput bind:value={draftMusical.bpm} min={20} max={400} unit="BPM" />
					</Field>
					<Field label={$t('projectSettings.timeSignature')}>
						<TimeSignatureInput
							bind:numerator={draftMusical.numerator}
							bind:denominator={draftMusical.denominator}
						/>
					</Field>
				</FieldGrid>
				<Field label={$t('projectSettings.midiPpq')} hint={$t('projectSettings.midiPpqHint')}>
					<PillGroup
						bind:value={draftMusical.midi_ppq}
						options={MIDI_PPQ_OPTIONS.map((o) => ({ value: o.value, label: o.label }))}
					/>
				</Field>
			</SubSection>

			<SubSection title={$t('projectSettings.keyGroove')}>
				<FieldGrid>
					<Field hint={$t('projectSettings.keyHint')}>
						<svelte:fragment slot="label">
							{$t('projectSettings.key')}
							<span class="badge-soon">{$t('common.comingSoon')}</span>
						</svelte:fragment>
						<div class="field-row">
							<CustomSelect
								small
								bind:value={draftMusical.key_root}
								options={KEY_ROOT_OPTIONS.map((r) => ({ value: r, label: r }))}
							/>
							<CustomSelect
								small
								bind:value={draftMusical.key_mode}
								options={[...KEY_MODE_OPTIONS]}
							/>
						</div>
					</Field>
					<Field hint={$t('projectSettings.swingHint')}>
						<svelte:fragment slot="label">
							{$t('projectSettings.swing')}
							<span class="badge-soon">{$t('common.comingSoon')}</span>
						</svelte:fragment>
						<NumberInput bind:value={draftMusical.swing_percent} min={0} max={100} unit="%" />
					</Field>
				</FieldGrid>
			</SubSection>

			{#if applyError}
				<p class="error-msg">{applyError}</p>
			{/if}
		</div>

		<DialogFooter>
			<Button variant="secondary" on:click={cancelAndClose}>{$t('common.cancel')}</Button>
			<Button variant="primary" on:click={applyAndClose}>{$t('common.save')}</Button>
		</DialogFooter>
	</Dialog>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.badge-soon {
		font-size: 9px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: $text-subtle;
		background: $surface-2;
		border-radius: $radius-xs;
		padding: 1px 5px;
		margin-left: 4px;
		vertical-align: middle;
	}

	.panel-body {
		padding: 20px 22px;
		overflow-y: auto;
		max-height: min(78vh, 560px);
	}

	.field-row {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.path-display {
		font-size: 12px;
		font-family: $font-mono;
		color: $text-muted;
		word-break: break-all;
	}

	.error-msg {
		font-size: 12px;
		color: $color-error;
		margin: 8px 0 0;
	}
</style>
