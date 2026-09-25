<script lang="ts">
	import PluginPicker from './PluginPicker.svelte';
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
		UNAVAILABLE_REASON_LABEL,
		type UnavailableReason,
	} from '../stores/plugins';
	import { invoke } from '@tauri-apps/api/core';
	import { setTrackInstrument } from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';

	export let trackId: string;
	export let instrument: string | null;
	export let compact = false;
	export let trackName = '';
	export let trackColor = '#4d96ff';

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

	$: activePlugin = instrument
		? $plugins.find((p) => p.id === instrument)
		: null;

	$: unavailable = instrument ? $unavailablePlugins.get(editorKey) : undefined;

	$: label = activePlugin
		? activePlugin.name
		: instrument
			? pluginDisplayName(instrument)
			: 'PolySynth (intégré)';

	$: canOpenEditor = !!instrument && !!activePlugin && !unavailable;

	$: isolation = $preferences.audio.pluginIsolation;
	$: isClap = activePlugin?.format === 'CLAP';
	/** Whether the engine runs this instrument in its own process, given the preferences. */
	$: sandboxed =
		!!instrument &&
		isClap &&
		(isolation === 'all' || (isolation === 'chosen' && $preferences.audio.alwaysIsolated.includes(instrument)));

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
			"Echoes ne sait pas encore utiliser ce format comme instrument : la piste est muette. Choisissez un instrument CLAP.",
		crashed:
			"Le plugin a planté : la piste est muette. Réessayez pour le recharger.",
		load_failed:
			"Le plugin n'a pas pu démarrer : la piste est muette. Ses réglages sont conservés.",
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
					pluginName: activePlugin.name,
				});
				editorOpen = !sandboxed;
			}
		} catch (e) {
			notify(`Erreur éditeur : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	const FORMAT_COLOR: Record<string, string> = {
		VST3: '#1571ff',
		CLAP: '#22c55e',
		AU: '#f59e0b',
	};
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
			<span class="instr-title">Instrument</span>
		{/if}
		<button class="change-btn" type="button" on:click={() => (showPicker = true)} title="Changer l'instrument">
			{compact ? '…' : 'Changer'}
		</button>
	</div>

	<!-- svelte-ignore a11y-no-static-element-interactions -->
	<div class="instr-slot" class:unavailable on:contextmenu={openContextMenu}>
		<span class="instr-name" title={instrument ?? label}>{label}</span>
		{#if sandboxed}
			<span class="sandbox-badge" title="Isolé : un plantage de ce plugin ne ferme pas Echoes">ISO</span>
		{/if}
		{#if unavailable}
			<span class="missing-badge" title={UNAVAILABLE_HELP[unavailable.reason]}>
				{UNAVAILABLE_REASON_LABEL[unavailable.reason]}
			</span>
		{/if}
		{#if activePlugin}
			<span
				class="fmt-badge"
				style:background={FORMAT_COLOR[activePlugin.format] ?? '#555'}
			>{activePlugin.format}</span>
		{/if}
		{#if canOpenEditor}
			<button
				class="gui-btn"
				class:active={editorOpen}
				type="button"
				on:click={handleToggleEditor}
				title={editorOpen ? "Fermer l'éditeur" : "Ouvrir l'éditeur"}
			>GUI</button>
		{/if}
		{#if instrument}
			<button class="rm-btn" type="button" on:click={handleClear} title="PolySynth intégré">×</button>
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
	{:else if !compact && !instrument}
		<p class="hint">Synthé intégré par défaut — choisissez un instrument CLAP (ex. Vital).</p>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.instr-panel {
		width: 100%;

		&.compact .instr-header {
			padding: 4px 5px 4px 0;
		}

		&.compact .instr-slot {
			padding: 4px 5px;
		}

		&.compact .instr-name {
			font-size: 10px;
		}

		&.compact .change-btn {
			min-width: 18px;
			padding: 2px 5px;
		}
	}

	.instr-header {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 5px 6px;
		border-bottom: 1px solid $border-faint;
		background: #222229;
	}

	.track-bar {
		width: 3px;
		height: 14px;
		background: var(--track-color, $accent-color);
		border-radius: 0 2px 2px 0;
		flex-shrink: 0;
	}

	.track-name {
		flex: 1;
		min-width: 0;
		font-size: 10px;
		font-weight: 600;
		color: $text-color;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.instr-title {
		flex: 1;
		font-size: 10px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.08em;
		color: $text-muted;
		text-transform: uppercase;
	}

	.change-btn {
		font-size: 10px;
		font-family: $font-mono;
		padding: 3px 7px;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		background: transparent;
		color: $text-subtle;
		cursor: pointer;
		&:hover {
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;
			border-color: rgba(21, 113, 255, 0.4);
		}
	}

	.instr-slot {
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 8px 6px;
		border-bottom: 1px solid $border-faint;
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
		font-size: 7px;
		font-family: $font-mono;
		font-weight: 700;
		letter-spacing: 0.04em;
		padding: 1px 4px;
		border-radius: 2px;
		color: #fff;
		flex-shrink: 0;
	}

	.sandbox-badge {
		font-size: 7px;
		font-family: $font-mono;
		font-weight: 700;
		padding: 1px 4px;
		border-radius: 2px;
		flex-shrink: 0;
		border: 1px solid rgba(34, 197, 94, 0.5);
		background: rgba(34, 197, 94, 0.12);
		color: #4ade80;
	}

	.gui-btn {
		flex-shrink: 0;
		padding: 1px 5px;
		border: 1px solid $border-subtle;
		border-radius: 3px;
		background: transparent;
		color: $text-subtle;
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		cursor: pointer;
		&:hover {
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;
		}
		&.active {
			background: rgba(21, 113, 255, 0.2);
			color: $accent-color;
			border-color: rgba(21, 113, 255, 0.5);
		}
	}

	.rm-btn {
		width: 16px;
		height: 16px;
		border: none;
		background: transparent;
		color: $text-faint;
		font-size: 14px;
		line-height: 1;
		cursor: pointer;
		padding: 0;
		flex-shrink: 0;
		&:hover {
			color: #ff4136;
		}
	}

	.instr-slot.unavailable .instr-name {
		color: $text-faint;
		text-decoration: line-through;
	}

	.missing-badge {
		flex-shrink: 0;
		padding: 1px 5px;
		border: 1px solid rgba(255, 65, 54, 0.5);
		border-radius: 3px;
		background: rgba(255, 65, 54, 0.12);
		color: #ff6b61;
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		white-space: nowrap;
	}

	.missing-row {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 6px;
		padding: 6px 8px 8px;
		border-bottom: 1px solid $border-faint;
	}

	.missing-help {
		margin: 0;
		font-size: 9px;
		line-height: 1.35;
		color: $text-subtle;
	}

	.retry-btn {
		padding: 3px 10px;
		border: 1px solid $border-subtle;
		border-radius: 4px;
		background: transparent;
		color: $text-subtle;
		font-size: 10px;
		font-family: $font-mono;
		cursor: pointer;
		&:hover:not(:disabled) {
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;
			border-color: rgba(21, 113, 255, 0.4);
		}
		&:disabled {
			cursor: default;
			opacity: 0.6;
		}
	}

	.hint {
		font-size: 9px;
		color: $text-faint;
		margin: 0;
		padding: 6px 8px 8px;
		line-height: 1.35;
	}

	.editor-row {
		display: flex;
		justify-content: center;
		padding: 8px 6px;
		border-bottom: 1px solid $border-faint;
	}

	.open-editor-btn {
		padding: 5px 14px;
		border: 1px solid $border-subtle;
		border-radius: 4px;
		background: transparent;
		color: $text-subtle;
		font-size: 10px;
		font-family: $font-mono;
		font-weight: 600;
		letter-spacing: 0.04em;
		cursor: pointer;
		transition: background 0.1s, color 0.1s, border-color 0.1s;
		&:hover {
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;
			border-color: rgba(21, 113, 255, 0.4);
		}
		&.active {
			background: rgba(21, 113, 255, 0.2);
			color: $accent-color;
			border-color: rgba(21, 113, 255, 0.5);
		}
	}
</style>
