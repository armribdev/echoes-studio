<!-- EffectChain.svelte — per-track insert effects panel -->
<script lang="ts">
	import PluginPicker from './PluginPicker.svelte';
	import EffectContextMenu from './EffectContextMenu.svelte';
	import EffectKnobs from './effects/EffectKnobs.svelte';
	import EffectPluginBody from './effects/EffectPluginBody.svelte';
	import EffectSlotHead from './effects/EffectSlotHead.svelte';
	import { modulationDrag } from '../stores/modulationDrag';
	import { slide } from 'svelte/transition';
	import type {
		TrackEffect,
		PluginEffectParams,
		ProjectModulator,
		ProjectModulationRoute
	} from '../types/global';
	import type { PluginInfo } from '../stores/plugins';
	import {
		loadPlugins,
		plugins,
		rescanPlugins,
		unavailablePlugins,
		type UnavailableReason
	} from '../stores/plugins';
	import { addEffect, removeEffect, updateEffect } from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';
	import { preferences } from '../stores/preferences';
	import { invoke } from '@tauri-apps/api/core';
	import { t } from 'svelte-i18n';

	const REASON_KEYS: Record<UnavailableReason, string> = {
		not_installed: 'missingPlugins.notInstalled',
		unsupported_format: 'missingPlugins.unsupportedFormat',
		load_failed: 'missingPlugins.loadFailed',
		crashed: 'missingPlugins.crashed'
	};

	export let trackId: string;
	export let effects: TrackEffect[];
	export let compact = false;
	export let modulators: ProjectModulator[] = [];
	export let mod_routes: ProjectModulationRoute[] = [];

	let expanded: Record<string, boolean> = {};
	let showPicker = false;
	// effect_ids that currently have an open native editor window
	let openEditors: Set<string> = new Set();

	// Ensure plugins are loaded (no-op if already loaded)
	if ($plugins.length === 0) loadPlugins();

	function toggleExpand(id: string) {
		expanded[id] = !expanded[id];
		expanded = expanded;
	}
	async function handlePickPlugin(p: PluginInfo) {
		showPicker = false;
		const kind = p.format === 'Internal' ? p.id : `plugin:${p.id}|${p.name}|${p.format}`;
		try {
			await addEffect(trackId, kind);
		} catch (e) {
			const msg = (e as Error)?.message ?? e;
			notify(`Erreur ajout ${p.format === 'Internal' ? 'effet' : 'plugin'} : ${msg}`, 'error');
		}
	}

	async function handleRemoveEffect(effectId: string) {
		try {
			await removeEffect(trackId, effectId);
		} catch (e) {
			notify(`Erreur suppression : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function handleToggle(effect: TrackEffect) {
		try {
			await updateEffect(trackId, effect.id, { enabled: !effect.enabled });
		} catch (e) {
			notify(`Erreur bypass : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function handleParam(effectId: string, params: Record<string, unknown>) {
		try {
			await updateEffect(trackId, effectId, { params });
		} catch (e) {
			notify(`Erreur paramètre : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	$: isolation = $preferences.audio.pluginIsolation;
	$: alwaysIsolated = $preferences.audio.alwaysIsolated;

	function pluginId(effect: TrackEffect): string | null {
		return effect.params.kind === 'plugin' ? effect.params.params.plugin_id : null;
	}

	function hasSidechainInput(effect: TrackEffect, installed = $plugins): boolean {
		if (effect.params.kind !== 'plugin') return false;
		const { plugin_id, sidechain_source } = effect.params.params;
		const info = installed.find((p) => p.id === plugin_id);
		return (info?.num_sidechain_inputs ?? 0) > 0 || !!sidechain_source;
	}

	function isChosen(effect: TrackEffect): boolean {
		return effect.params.kind === 'plugin' && effect.params.params.sandboxed;
	}

	function isAlwaysIsolated(effect: TrackEffect, always = alwaysIsolated): boolean {
		const id = pluginId(effect);
		return id !== null && always.includes(id);
	}

	function canBeIsolated(effect: TrackEffect): boolean {
		const format = effectFormat(effect);
		return format === 'CLAP' || format === 'VST3';
	}

	/** Whether the engine runs this effect in its own process, given the preferences. */
	function isSandboxed(effect: TrackEffect, mode = isolation, always = alwaysIsolated): boolean {
		if (!canBeIsolated(effect) || mode === 'off') return false;
		return mode === 'all' || isChosen(effect) || isAlwaysIsolated(effect, always);
	}

	let contextMenu: { x: number; y: number; pluginId: string; pluginName: string } | null = null;

	function openContextMenu(event: MouseEvent, effect: TrackEffect) {
		const id = pluginId(effect);
		if (id === null || !canBeIsolated(effect)) return;
		event.preventDefault();
		contextMenu = {
			x: event.clientX,
			y: event.clientY,
			pluginId: id,
			pluginName: effectLabel(effect)
		};
	}

	async function handleToggleSandbox(effect: TrackEffect) {
		try {
			if (openEditors.has(effect.id)) {
				await invoke('close_plugin_editor', { effectId: effect.id });
				openEditors.delete(effect.id);
				openEditors = new Set(openEditors);
			}
			await updateEffect(trackId, effect.id, { sandboxed: !isChosen(effect) });
		} catch (e) {
			notify(`Erreur isolation : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function handleOpenEditor(effect: TrackEffect) {
		if (effect.params.kind !== 'plugin') return;
		const p: PluginEffectParams = effect.params.params;
		// An isolated plugin's editor is its own window: opening it again brings it to the front.
		const isOpen = !isSandboxed(effect) && openEditors.has(effect.id);
		try {
			if (isOpen) {
				await invoke('close_plugin_editor', { effectId: effect.id });
				openEditors.delete(effect.id);
			} else {
				await invoke('open_plugin_editor', {
					pluginId: p.plugin_id,
					effectId: effect.id,
					pluginName: p.name ?? 'Plugin'
				});
				if (!isSandboxed(effect)) openEditors.add(effect.id);
			}
			openEditors = new Set(openEditors); // trigger reactivity
		} catch (e) {
			notify(`Erreur éditeur plugin : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	const UNAVAILABLE_HELP: Record<UnavailableReason, string> = {
		not_installed:
			"Ce plugin n'est pas installé sur cet ordinateur : le son passe sans être traité. Ses réglages sont conservés. Réinstallez-le puis réessayez.",
		unsupported_format:
			'Echoes ne sait pas encore utiliser ce format : le son passe sans être traité.',
		load_failed:
			"Le plugin n'a pas pu démarrer : le son passe sans être traité. Ses réglages sont conservés.",
		crashed:
			'Le plugin a planté : son son est coupé, Echoes continue. Rechargez-le ; il reprend ses derniers réglages enregistrés.'
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

	let reloading = false;

	async function handleReload() {
		reloading = true;
		try {
			await invoke('reload_plugins');
		} catch (e) {
			notify(`Erreur rechargement : ${(e as Error)?.message ?? e}`, 'error');
		} finally {
			reloading = false;
		}
	}

	const BUILTIN_LABELS: Record<string, string> = {
		eq: 'EQ 3-bandes',
		compressor: 'Compresseur',
		reverb: 'Réverb',
		delay: 'Délai'
	};

	function effectLabel(effect: TrackEffect): string {
		if (effect.params.kind === 'plugin') return effect.params.params.name ?? 'Plugin';
		return BUILTIN_LABELS[effect.params.kind] ?? effect.params.kind;
	}

	function effectFormat(effect: TrackEffect): string | null {
		if (effect.params.kind === 'plugin') return effect.params.params.format ?? null;
		return null;
	}
</script>

{#if contextMenu}
	<EffectContextMenu
		x={contextMenu.x}
		y={contextMenu.y}
		pluginId={contextMenu.pluginId}
		pluginName={contextMenu.pluginName}
		on:close={() => (contextMenu = null)}
	/>
{/if}

{#if showPicker}
	<PluginPicker
		mode="effect"
		on:pick={(e) => handlePickPlugin(e.detail)}
		on:cancel={() => (showPicker = false)}
	/>
{/if}

<div class="fx-panel" class:compact>
	<div class="fx-header">
		<span class="fx-title">{compact ? 'FX' : 'Effets'}</span>
		<button
			class="add-btn"
			type="button"
			on:click|stopPropagation={() => (showPicker = true)}
			title={$t('effects.add')}>+</button
		>
	</div>

	{#if effects.length === 0}
		<p class="empty">{compact ? '—' : 'Aucun effet — cliquez + pour en ajouter'}</p>
	{/if}

	{#each effects as effect (effect.id)}
		{@const unavailable = $unavailablePlugins.get(effect.id)}
		<div
			class="slot"
			class:bypassed={!effect.enabled}
			class:unavailable
			class:crashed={unavailable?.reason === 'crashed'}
		>
			<EffectSlotHead
				label={effectLabel(effect)}
				format={effectFormat(effect)}
				enabled={effect.enabled}
				isolated={isSandboxed(effect, isolation, alwaysIsolated)}
				missingLabel={unavailable ? $t(REASON_KEYS[unavailable.reason]) : null}
				missingHelp={unavailable ? UNAVAILABLE_HELP[unavailable.reason] : ''}
				crashed={unavailable?.reason === 'crashed'}
				open={expanded[effect.id] || !!$modulationDrag}
				{reloading}
				{compact}
				on:contextmenu={(e) => openContextMenu(e, effect)}
				on:toggleBypass={() => handleToggle(effect)}
				on:toggleExpand={() => toggleExpand(effect.id)}
				on:reload={handleReload}
				on:remove={() => handleRemoveEffect(effect.id)}
			/>

			{#if expanded[effect.id] || $modulationDrag}
				<div class="slot-params" transition:slide={{ duration: 180 }}>
					{#if effect.params.kind in BUILTIN_LABELS}
						<EffectKnobs
							{trackId}
							{effect}
							{compact}
							{modulators}
							{mod_routes}
							on:change={(e) => handleParam(effect.id, e.detail)}
						/>
					{:else if unavailable || effect.params.kind === 'plugin'}
						<EffectPluginBody
							{trackId}
							{effect}
							{compact}
							format={effectFormat(effect)}
							missingHelp={unavailable ? UNAVAILABLE_HELP[unavailable.reason] : null}
							canRetry={!!unavailable &&
								unavailable.reason !== 'unsupported_format' &&
								unavailable.reason !== 'crashed'}
							{retrying}
							editorOpen={openEditors.has(effect.id)}
							isolatable={canBeIsolated(effect) && isolation === 'chosen'}
							alwaysIsolated={isAlwaysIsolated(effect, alwaysIsolated)}
							chosenIsolated={isChosen(effect)}
							sandboxed={isSandboxed(effect, isolation, alwaysIsolated)}
							hasSidechain={hasSidechainInput(effect, $plugins)}
							on:retry={handleRetry}
							on:openEditor={() => handleOpenEditor(effect)}
							on:toggleSandbox={() => handleToggleSandbox(effect)}
						/>
					{/if}
				</div>
			{/if}
		</div>
	{/each}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.fx-panel {
		width: 100%;

		&.compact .fx-header {
			padding: 3px 5px;
		}
		&.compact .slot-params {
			padding: 4px 5px 5px;
		}
		&.compact .empty {
			padding: 6px 5px;
			font-size: 9px;
		}
	}

	.fx-header {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 5px 6px;
		border-bottom: 1px solid $border-faint;
		position: sticky;
		top: 0;
		background: $panel-background-color;
		z-index: 1;
	}

	.fx-title {
		flex: 1;
		font-size: 10px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.08em;
		color: $text-muted;
		text-transform: uppercase;
	}

	.add-btn {
		width: 18px;
		height: 18px;
		border: 1px solid $border-subtle;
		border-radius: $radius-xs;
		background: transparent;
		color: $text-subtle;
		font-size: 14px;
		line-height: 1;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 0;
		&:hover {
			background: $accent-soft;
			color: $text-color;
			border-color: $accent-border;
		}
	}

	.empty {
		font-size: 10px;
		color: $text-faint;
		text-align: center;
		padding: 12px 8px;
		margin: 0;
	}

	.slot {
		border-bottom: 1px solid $border-faint;
		&.bypassed {
			opacity: 0.4;
		}
	}

	.slot.crashed {
		border-left: 2px solid $color-error;
		background: $error-soft;
		opacity: 1;
	}

	.slot-params {
		padding: 6px 8px 8px;
		background: $black-wash;
	}
</style>
