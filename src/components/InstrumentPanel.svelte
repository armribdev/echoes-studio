<script lang="ts">
	import { DEFAULT_TRACK_COLOR } from '../frontend/trackColor';
	import { pluginFormatColor } from '../frontend/pluginFormatColor';
	import PluginPicker from './PluginPicker.svelte';
	import SamplerPads from './SamplerPads.svelte';
	import ArpeggiatorPanel from './ArpeggiatorPanel.svelte';
	import NoteEffectsPanel from './NoteEffectsPanel.svelte';
	import PresetPicker from './PresetPicker.svelte';
	import RemotePageKnobs from './RemotePageKnobs.svelte';
	import EffectContextMenu from './EffectContextMenu.svelte';
	import { preferences } from '../stores/preferences';
	import type { PluginInfo } from '../stores/plugins';
	import {
		instrumentIdOf,
		loadPlugins,
		plugins,
		pluginDisplayName,
		rescanPlugins,
		unavailablePlugins,
		type UnavailableReason
	} from '../stores/plugins';
	import { invoke } from '@tauri-apps/api/core';
	import { setTrackInstrument } from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';
	import { t } from 'svelte-i18n';
	import type { ArpeggiatorSettings, NoteEffectSettings, SamplerKit } from '../types/global';
	import { isSamplerInstrument } from '../shared/project/sampler';

	const REASON_KEYS: Record<UnavailableReason, string> = {
		not_installed: 'missingPlugins.notInstalled',
		unsupported_format: 'missingPlugins.unsupportedFormat',
		load_failed: 'missingPlugins.loadFailed',
		crashed: 'missingPlugins.crashed'
	};

	export let trackId: string;
	export let instrument: string | null;
	export let sampler: SamplerKit | null = null;
	export let arpeggiator: ArpeggiatorSettings | null = null;
	export let noteEffects: NoteEffectSettings | null = null;
	export let compact = false;
	export let trackName = '';
	export let trackColor = DEFAULT_TRACK_COLOR;

	let showPicker = false;
	let editorOpen = false;

	$: editorKey = `instrument:${trackId}`;

	let prevEditorKey = '';
	$: if (editorKey !== prevEditorKey) {
		if (prevEditorKey && editorOpen) {
			invoke('close_plugin_editor', { effectId: prevEditorKey }).catch(() => {});
		}
		editorOpen = false;
		prevEditorKey = editorKey;
	}

	if ($plugins.length === 0) loadPlugins();

	$: activePlugin = instrument ? $plugins.find((p) => p.id === instrument) : null;

	$: unavailable = instrument ? $unavailablePlugins.get(editorKey) : undefined;

	$: isSampler = isSamplerInstrument(instrument);

	$: label = activePlugin
		? activePlugin.name
		: isSampler
			? 'Sampler'
			: instrument
				? pluginDisplayName(instrument)
				: 'PolySynth (intégré)';

	$: canOpenEditor = !!instrument && !!activePlugin && !unavailable;

	$: isolation = $preferences.audio.pluginIsolation;
	$: isClap = activePlugin?.format === 'CLAP';
	$: hasPresets = isClap || activePlugin?.format === 'VST3';
	/** Whether the engine runs this instrument in its own process, given the preferences. */
	$: sandboxed =
		!!instrument &&
		isClap &&
		(isolation === 'all' ||
			(isolation === 'chosen' && $preferences.audio.alwaysIsolated.includes(instrument)));

	let contextMenu: { x: number; y: number } | null = null;

	function openContextMenu(event: MouseEvent) {
		if (!instrument || !isClap) return;
		event.preventDefault();
		contextMenu = { x: event.clientX, y: event.clientY };
	}

	const UNAVAILABLE_HELP: Record<UnavailableReason, string> = {
		not_installed:
			"Ce plugin n'est pas installé sur cet ordinateur : la piste est muette. Ses réglages sont conservés. Réinstallez-le puis réessayez, ou choisissez un autre instrument.",
		unsupported_format:
			'Echoes ne sait pas encore utiliser ce format comme instrument : la piste est muette. Choisissez un instrument CLAP.',
		crashed: 'Le plugin a planté : la piste est muette. Réessayez pour le recharger.',
		load_failed: "Le plugin n'a pas pu démarrer : la piste est muette. Ses réglages sont conservés."
	};

	let retrying = false;

	async function handleRetry() {
		retrying = true;
		try {
			await rescanPlugins();
		} finally {
			retrying = false;
		}
	}

	async function handlePick(p: PluginInfo) {
		showPicker = false;
		try {
			await setTrackInstrument(trackId, instrumentIdOf(p));
		} catch (e) {
			notify(`Erreur instrument : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function handleClear() {
		if (editorOpen) {
			try {
				await invoke('close_plugin_editor', { effectId: editorKey });
			} catch {
				/* ignore */
			}
			editorOpen = false;
		}
		try {
			await setTrackInstrument(trackId, null);
		} catch (e) {
			notify(`Erreur instrument : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function handleToggleEditor() {
		if (!canOpenEditor || !instrument || !activePlugin) return;
		try {
			// An isolated plugin's editor is its own window: opening it again brings it to the front.
			if (editorOpen && !sandboxed) {
				await invoke('close_plugin_editor', { effectId: editorKey });
				editorOpen = false;
			} else {
				await invoke('open_plugin_editor', {
					pluginId: instrument,
					effectId: editorKey,
					pluginName: activePlugin.name
				});
				editorOpen = !sandboxed;
			}
		} catch (e) {
			notify(`Erreur éditeur : ${(e as Error)?.message ?? e}`, 'error');
		}
	}
</script>

{#if showPicker}
	<PluginPicker
		mode="instrument"
		on:pick={(e) => handlePick(e.detail)}
		on:cancel={() => (showPicker = false)}
	/>
{/if}

{#if contextMenu && instrument}
	<EffectContextMenu
		x={contextMenu.x}
		y={contextMenu.y}
		pluginId={instrument}
		pluginName={label}
		on:close={() => (contextMenu = null)}
	/>
{/if}

<div class="instr-panel" class:compact>
	<div class="instr-header" style:--track-color={trackColor}>
		{#if compact}
			<span class="track-bar"></span>
			<span class="track-name">{trackName || 'Piste'}</span>
		{:else}
			<span class="instr-title">{$t('instrument.title')}</span>
		{/if}
		<button
			class="change-btn"
			type="button"
			on:click={() => (showPicker = true)}
			title={$t('instrument.change')}
		>
			{compact ? '…' : 'Changer'}
		</button>
	</div>

	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div class="instr-slot" class:unavailable on:contextmenu={openContextMenu}>
		<span class="instr-name" title={instrument ?? label}>{label}</span>
		{#if sandboxed}
			<span class="sandbox-badge" title={$t('instrument.isolated')}>ISO</span>
		{/if}
		{#if unavailable}
			<span class="missing-badge" title={UNAVAILABLE_HELP[unavailable.reason]}>
				{$t(REASON_KEYS[unavailable.reason])}
			</span>
		{/if}
		{#if activePlugin}
			<span class="fmt-badge" style:background={pluginFormatColor(activePlugin.format)}
				>{activePlugin.format}</span
			>
		{/if}
		{#if canOpenEditor}
			<button
				class="gui-btn"
				class:active={editorOpen}
				type="button"
				on:click={handleToggleEditor}
				title={editorOpen ? "Fermer l'éditeur" : "Ouvrir l'éditeur"}>GUI</button
			>
		{/if}
		{#if instrument}
			<button
				class="rm-btn"
				type="button"
				on:click={handleClear}
				title={$t('instrument.builtinPoly')}>×</button
			>
		{/if}
	</div>

	{#if unavailable}
		<div class="missing-row">
			{#if !compact}
				<p class="missing-help">{UNAVAILABLE_HELP[unavailable.reason]}</p>
			{/if}
			{#if unavailable.reason !== 'unsupported_format'}
				<button class="retry-btn" type="button" on:click={handleRetry} disabled={retrying}>
					{retrying ? 'Recherche…' : 'Réessayer'}
				</button>
			{/if}
		</div>
	{:else if !compact && canOpenEditor}
		<div class="editor-row">
			<button
				class="open-editor-btn"
				class:active={editorOpen}
				type="button"
				on:click={handleToggleEditor}
			>
				{editorOpen ? "Fermer l'éditeur" : "Ouvrir l'éditeur"}
			</button>
		</div>
	{:else if isSampler && sampler}
		<SamplerPads {trackId} kit={sampler} />
	{:else if !compact && !instrument}
		<p class="hint">Synthé intégré par défaut — choisissez un instrument CLAP (ex. Vital).</p>
	{/if}

	{#if canOpenEditor && (hasPresets || isClap)}
		<div class="instr-extras">
			{#if hasPresets}
				<PresetPicker instanceKey={editorKey} />
			{/if}
			{#if isClap}
				<RemotePageKnobs instanceKey={editorKey} />
			{/if}
		</div>
	{/if}

	<NoteEffectsPanel {trackId} {noteEffects} />
	<ArpeggiatorPanel {trackId} {arpeggiator} />
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.instr-panel {
		width: 100%;

		&.compact .change-btn {
			min-width: 18px;
			padding: $space-1 $space-3;
		}
	}

	.instr-header {
		display: flex;
		align-items: center;
		gap: $space-2;
		padding: $space-2 $panel-inset $space-2 0;
		background: $panel-background-color;
	}

	.track-bar {
		width: 3px;
		height: 14px;
		background: var(--track-color, $accent-color);
		border-radius: 0 2px 2px 0;
		flex-shrink: 0;
		margin-right: $panel-inset - 3px - $space-2;
	}

	.track-name {
		flex: 1;
		min-width: 0;
		font-size: 12px;
		font-weight: 600;
		color: $text-color;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.instr-title {
		flex: 1;
		font-size: 11px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.08em;
		color: $text-secondary;
		text-transform: uppercase;
	}

	.change-btn {
		@include chip($ctrl-height-xs);
	}

	.instr-slot {
		display: flex;
		align-items: center;
		gap: $space-2;
		padding: $space-3 $panel-inset;
	}

	.instr-name {
		flex: 1;
		font-size: 11px;
		font-weight: 500;
		color: $text-color;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.fmt-badge {
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		letter-spacing: 0.04em;
		padding: $space-1 $space-2;
		border-radius: 2px;
		color: $on-solid;
		flex-shrink: 0;
	}

	.sandbox-badge {
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		padding: $space-1 $space-2;
		border-radius: 2px;
		flex-shrink: 0;
		border: 1px solid $success-border;
		background: $success-soft;
		color: $color-success;
	}

	.gui-btn {
		@include chip;
		flex-shrink: 0;
		padding: 0 $space-3;

		&.active {
			@include control-on;
		}
	}

	.rm-btn {
		@include icon-button($ctrl-height-xs);
		@include icon-button-danger;
		font-size: 14px;
	}

	.instr-slot.unavailable .instr-name {
		color: $text-faint;
		text-decoration: line-through;
	}

	.missing-badge {
		flex-shrink: 0;
		padding: $space-1 $space-2;
		border: 1px solid $error-border;
		border-radius: $radius-xs;
		background: $error-soft;
		color: $color-error-text;
		font-size: 9px;
		font-family: $font-mono;
		font-weight: 700;
		white-space: nowrap;
	}

	.missing-row {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: $space-3;
		padding: $space-3 $panel-inset;
	}

	.missing-help {
		margin: 0;
		font-size: 9px;
		line-height: 1.35;
		color: $text-subtle;
	}

	.retry-btn {
		@include chip($ctrl-height-sm);
	}

	.instr-extras {
		display: flex;
		flex-direction: column;
		gap: $space-3;
		padding: $space-3 $panel-inset;
	}

	.hint {
		font-size: 9px;
		color: $text-faint;
		margin: 0;
		padding: $space-3 $panel-inset;
		line-height: 1.35;
	}

	.editor-row {
		display: flex;
		justify-content: center;
		padding: $space-3 $panel-inset;
	}

	.open-editor-btn {
		@include chip($ctrl-height-sm);
		padding: 0 $space-6;
		font-weight: 600;

		&.active {
			@include control-on;
		}
	}
</style>
