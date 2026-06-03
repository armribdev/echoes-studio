<script lang="ts">
	import PluginPicker from './PluginPicker.svelte';
	import type { PluginInfo } from '../stores/plugins';
	import { loadPlugins, plugins } from '../stores/plugins';
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

	$: label = activePlugin
		? activePlugin.name
		: 'PolySynth (intégré)';

	async function handlePick(p: PluginInfo) {
		showPicker = false;
		try {
			await setTrackInstrument(trackId, p.id);
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
		if (!instrument || !activePlugin) return;
		try {
			if (editorOpen) {
				await invoke('close_plugin_editor', { effectId: editorKey });
				editorOpen = false;
			} else {
				await invoke('open_plugin_editor', {
					pluginId: instrument,
					effectId: editorKey,
					pluginName: activePlugin.name,
				});
				editorOpen = true;
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

	<div class="instr-slot">
		<span class="instr-name" title={label}>{label}</span>
		{#if activePlugin}
			<span
				class="fmt-badge"
				style:background={FORMAT_COLOR[activePlugin.format] ?? '#555'}
			>{activePlugin.format}</span>
		{/if}
		{#if instrument && activePlugin}
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

	{#if !compact && instrument && activePlugin}
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
