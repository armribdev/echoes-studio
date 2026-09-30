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
	import { dismissable } from '../frontend/dismiss';
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
	<div class="overlay">
		<div
			class="panel"
			role="dialog"
			aria-modal="true"
			aria-label={$t('projectSettings.aria')}
			use:dismissable={cancelAndClose}
		>
			<div class="panel-header">
				<span class="header-icon"><Icon name="folder" size={14} /></span>
				<span class="panel-title">{$t('projectSettings.title')}</span>
				<button class="close-btn" on:click={cancelAndClose} title={$t('common.closeEsc')}>
					<Icon name="close" size={15} strokeWidth={1.5} />
				</button>
			</div>

			<div class="panel-body">
				<div class="section">
					<h2 class="section-title">{$t('projectSettings.identity')}</h2>

					<div class="field">
						<span class="field-label">{$t('projectSettings.projectName')}</span>
						<input
							class="text-input"
							type="text"
							bind:value={draftName}
							placeholder={$t('common.untitled')}
							maxlength="120"
						/>
					</div>

					<div class="field">
						<span class="field-label">{$t('projectSettings.file')}</span>
						<span class="path-display" title={$projectPath ?? undefined}>
							{formatPath($projectPath)}
						</span>
					</div>
				</div>

				<div class="divider"></div>

				<div class="section">
					<h2 class="section-title">{$t('projectSettings.musical')}</h2>
					<p class="section-hint">{$t('projectSettings.musicalHint')}</p>

					<div class="field-grid">
						<div class="field">
							<span class="field-label">{$t('projectSettings.tempo')}</span>
							<NumberInput bind:value={draftMusical.bpm} min={20} max={400} unit="BPM" />
						</div>
						<div class="field">
							<span class="field-label">{$t('projectSettings.timeSignature')}</span>
							<div class="field-row">
								<NumberInput bind:value={draftMusical.numerator} min={1} max={16} width="42px" />
								<span class="field-slash">/</span>
								<CustomSelect
									small
									bind:value={draftMusical.denominator}
									options={[
										{ value: 2, label: '2' },
										{ value: 4, label: '4' },
										{ value: 8, label: '8' },
										{ value: 16, label: '16' }
									]}
								/>
							</div>
						</div>
					</div>

					<div class="field">
						<span class="field-label">{$t('projectSettings.midiPpq')}</span>
						<PillGroup
							bind:value={draftMusical.midi_ppq}
							options={MIDI_PPQ_OPTIONS.map((o) => ({ value: o.value, label: o.label }))}
						/>
						<span class="field-hint">{$t('projectSettings.midiPpqHint')}</span>
					</div>
				</div>

				<div class="divider"></div>

				<div class="section">
					<h2 class="section-title">{$t('projectSettings.keyGroove')}</h2>

					<div class="field-grid">
						<div class="field field--soon">
							<span class="field-label"
								>{$t('projectSettings.key')}
								<span class="badge-soon">{$t('common.comingSoon')}</span></span
							>
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
							<span class="field-hint">{$t('projectSettings.keyHint')}</span>
						</div>
						<div class="field field--soon">
							<span class="field-label"
								>{$t('projectSettings.swing')}
								<span class="badge-soon">{$t('common.comingSoon')}</span></span
							>
							<NumberInput bind:value={draftMusical.swing_percent} min={0} max={100} unit="%" />
							<span class="field-hint">{$t('projectSettings.swingHint')}</span>
						</div>
					</div>
				</div>

				{#if applyError}
					<p class="error-msg">{applyError}</p>
				{/if}
			</div>

			<div class="panel-footer">
				<div class="footer-actions">
					<Button variant="secondary" on:click={cancelAndClose}>{$t('common.cancel')}</Button>
					<Button variant="primary" on:click={applyAndClose}>{$t('common.save')}</Button>
				</div>
			</div>
		</div>
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.65);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 1000;
		backdrop-filter: blur(4px);
	}

	.field-hint {
		font-size: 11px;
		color: rgba(247, 247, 249, 0.35);
		line-height: 1.4;
	}

	.badge-soon {
		font-size: 9px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: rgba(247, 247, 249, 0.35);
		background: rgba(255, 255, 255, 0.06);
		border-radius: 3px;
		padding: 1px 5px;
		margin-left: 4px;
		vertical-align: middle;
	}

	.panel {
		width: 500px;
		max-width: calc(100vw - 48px);
		background: #1e1e24;
		border: 1px solid $border-color;
		border-radius: $radius-xl;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		box-shadow:
			0 24px 64px rgba(0, 0, 0, 0.7),
			0 0 0 0.5px rgba(255, 255, 255, 0.06);
	}

	.panel-header {
		display: flex;
		align-items: center;
		padding: 14px 18px;
		border-bottom: 1px solid $border-color;
		background: $panel-background-color;
		flex-shrink: 0;
		gap: 9px;
	}

	.header-icon {
		color: rgba(247, 247, 249, 0.35);
		flex-shrink: 0;
	}

	.panel-title {
		font-size: 13px;
		font-weight: 600;
		color: $text-color;
		flex: 1;
	}

	.close-btn {
		background: none;
		border: none;
		color: rgba(247, 247, 249, 0.4);
		width: 26px;
		height: 26px;
		border-radius: 4px;
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;

		&:hover {
			background: rgba(255, 255, 255, 0.1);
			color: $text-color;
		}
	}

	.panel-body {
		padding: 20px 22px;
		overflow-y: auto;
		max-height: min(78vh, 560px);
	}

	.section-title {
		font-size: 11px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.6px;
		color: rgba(247, 247, 249, 0.45);
		margin: 0 0 12px;
	}

	.section-hint {
		font-size: 11px;
		color: rgba(247, 247, 249, 0.35);
		margin: -8px 0 12px;
	}

	.field {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin-bottom: 14px;
	}

	.field-label {
		font-size: 11px;
		color: rgba(247, 247, 249, 0.5);
	}

	.field-grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 16px;
	}

	.field-row {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.field-slash {
		color: rgba(247, 247, 249, 0.3);
		font-size: 15px;
	}

	.text-input {
		height: 32px;
		padding: 0 10px;
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: $radius-sm;
		color: $text-color;
		font-size: 13px;
		font-family: inherit;

		&:focus {
			outline: none;
			border-color: $accent-color;
		}
	}

	.path-display {
		font-size: 12px;
		font-family: $font-mono;
		color: rgba(247, 247, 249, 0.55);
		word-break: break-all;
	}

	.divider {
		height: 1px;
		background: $border-color;
		margin: 4px 0 18px;
	}

	.error-msg {
		font-size: 12px;
		color: $color-error;
		margin: 8px 0 0;
	}

	.panel-footer {
		display: flex;
		justify-content: flex-end;
		padding: 12px 18px;
		border-top: 1px solid $border-color;
		background: $panel-background-color;
	}

	.footer-actions {
		display: flex;
		gap: 8px;
	}
</style>
