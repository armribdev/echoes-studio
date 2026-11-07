<!-- EffectChain.svelte — per-track insert effects panel -->
<script lang="ts">
	import Knob from './Knob.svelte';
	import PluginPicker from './PluginPicker.svelte';
	import PresetPicker from './PresetPicker.svelte';
	import SidechainPicker from './SidechainPicker.svelte';
	import EffectContextMenu from './EffectContextMenu.svelte';
	import { modulatorColor } from '../frontend/modulation/colors';
	import { modulationDrag } from '../stores/modulationDrag';
	import { automatedTargets, automatedValues } from '../stores/automation';
	import { selectedTrack } from '../stores/selectedTrack';
	import { slide } from 'svelte/transition';
	import type {
		TrackEffect,
		EqParams,
		CompressorParams,
		ReverbParams,
		DelayParams,
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

	$: automated = $automatedTargets[trackId];

	$: automatedNow = $automatedValues[trackId];

	function automatedValueFor(effectId: string, param: string): number | undefined {
		return automatedNow?.[`${effectId}/${param}`];
	}

	function automationColorFor(effectId: string, param: string): string | undefined {
		return automated?.has(`${effectId}/${param}`) ? $selectedTrack.trackColor : undefined;
	}

	function modColorFor(effectId: string, param: string): string | undefined {
		const route = mod_routes.find(
			(r) =>
				r.target.kind === 'effect_param' &&
				r.target.effect_id === effectId &&
				r.target.param === param
		);
		if (!route) return undefined;
		const idx = modulators.findIndex((m) => m.id === route.modulator_id);
		return modulatorColor(idx >= 0 ? idx : 0);
	}

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

	const FORMAT_COLOR: Record<string, string> = {
		VST3: '#1571ff',
		CLAP: '#22c55e',
		AU: '#f59e0b'
	};

	function eqP(e: TrackEffect) {
		return (e.params as { kind: 'eq'; params: EqParams }).params;
	}
	function compP(e: TrackEffect) {
		return (e.params as { kind: 'compressor'; params: CompressorParams }).params;
	}
	function revP(e: TrackEffect) {
		return (e.params as { kind: 'reverb'; params: ReverbParams }).params;
	}
	function delP(e: TrackEffect) {
		return (e.params as { kind: 'delay'; params: DelayParams }).params;
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
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<div class="slot-head" on:contextmenu={(e) => openContextMenu(e, effect)}>
				<button
					class="bypass"
					class:on={effect.enabled}
					type="button"
					on:click|stopPropagation={() => handleToggle(effect)}
					title={$t('effects.bypass')}
				>
					<svg width="8" height="8" viewBox="0 0 8 8"
						><circle cx="4" cy="4" r="3" fill="currentColor" /></svg
					>
				</button>
				<button class="slot-toggle" type="button" on:click={() => toggleExpand(effect.id)}>
					<span class="slot-name">{effectLabel(effect)}</span>
					{#if effectFormat(effect)}
						<span
							class="fmt-badge"
							style:background={FORMAT_COLOR[effectFormat(effect) ?? ''] ?? '#555'}
						>
							{effectFormat(effect)}
						</span>
					{/if}
					{#if isSandboxed(effect, isolation, alwaysIsolated)}
						<span class="sandbox-badge" title={$t('effects.isolated')}
							>ISO</span
						>
					{/if}
					{#if unavailable}
						<span class="missing-badge" title={UNAVAILABLE_HELP[unavailable.reason]}>
							{$t(REASON_KEYS[unavailable.reason])}
						</span>
					{/if}
					<svg
						class="chevron"
						class:open={expanded[effect.id] || $modulationDrag}
						width="8"
						height="5"
						viewBox="0 0 8 5"
					>
						<path
							d="M1 1 L4 4 L7 1"
							stroke="currentColor"
							stroke-width="1.5"
							fill="none"
							stroke-linecap="round"
						/>
					</svg>
				</button>
				{#if unavailable?.reason === 'crashed'}
					<button
						class="reload-btn"
						type="button"
						on:click|stopPropagation={handleReload}
						disabled={reloading}
						title={UNAVAILABLE_HELP.crashed}
					>
						{compact ? '↻' : reloading ? 'Rechargement…' : 'Recharger'}
					</button>
				{/if}
				<button
					class="rm-btn"
					type="button"
					on:click|stopPropagation={() => handleRemoveEffect(effect.id)}
					title={$t('common.remove')}>×</button
				>
			</div>

			{#if expanded[effect.id] || $modulationDrag}
				<div class="slot-params" transition:slide={{ duration: 180 }}>
					{#if effect.params.kind === 'eq'}
						{@const p = eqP(effect)}
						<div class="grid">
							<span class="lbl">LOW</span>
							<Knob
								min={-12}
								max={12}
								defaultValue={0}
								value={p.low_gain_db}
								modColor={modColorFor(effect.id, 'low_gain_db')}
								automationColor={automationColorFor(effect.id, 'low_gain_db')}
								automatedValue={automatedValueFor(effect.id, 'low_gain_db')}
								modTargetKey={`${effect.id}/low_gain_db`}
								on:release={(e) => handleParam(effect.id, { ...p, low_gain_db: e.detail })}
							/>
							<span class="val">{p.low_gain_db.toFixed(1)} dB</span>
							<span class="lbl">MID</span>
							<Knob
								min={-12}
								max={12}
								defaultValue={0}
								value={p.mid_gain_db}
								modColor={modColorFor(effect.id, 'mid_gain_db')}
								automationColor={automationColorFor(effect.id, 'mid_gain_db')}
								automatedValue={automatedValueFor(effect.id, 'mid_gain_db')}
								modTargetKey={`${effect.id}/mid_gain_db`}
								on:release={(e) => handleParam(effect.id, { ...p, mid_gain_db: e.detail })}
							/>
							<span class="val">{p.mid_gain_db.toFixed(1)} dB</span>
							<span class="lbl">HIGH</span>
							<Knob
								min={-12}
								max={12}
								defaultValue={0}
								value={p.high_gain_db}
								modColor={modColorFor(effect.id, 'high_gain_db')}
								automationColor={automationColorFor(effect.id, 'high_gain_db')}
								automatedValue={automatedValueFor(effect.id, 'high_gain_db')}
								modTargetKey={`${effect.id}/high_gain_db`}
								on:release={(e) => handleParam(effect.id, { ...p, high_gain_db: e.detail })}
							/>
							<span class="val">{p.high_gain_db.toFixed(1)} dB</span>
							<span class="lbl">Q</span>
							<Knob
								min={0.1}
								max={10}
								defaultValue={1}
								value={p.mid_q}
								modColor={modColorFor(effect.id, 'mid_q')}
								automationColor={automationColorFor(effect.id, 'mid_q')}
								automatedValue={automatedValueFor(effect.id, 'mid_q')}
								modTargetKey={`${effect.id}/mid_q`}
								on:release={(e) => handleParam(effect.id, { ...p, mid_q: e.detail })}
							/>
							<span class="val">{p.mid_q.toFixed(2)}</span>
						</div>
					{:else if effect.params.kind === 'compressor'}
						{@const p = compP(effect)}
						<div class="grid">
							<span class="lbl">THR</span>
							<Knob
								min={-60}
								max={0}
								defaultValue={-18}
								value={p.threshold_db}
								modColor={modColorFor(effect.id, 'threshold_db')}
								automationColor={automationColorFor(effect.id, 'threshold_db')}
								automatedValue={automatedValueFor(effect.id, 'threshold_db')}
								modTargetKey={`${effect.id}/threshold_db`}
								on:release={(e) => handleParam(effect.id, { ...p, threshold_db: e.detail })}
							/>
							<span class="val">{p.threshold_db.toFixed(1)} dB</span>
							<span class="lbl">RATIO</span>
							<Knob
								min={1}
								max={20}
								defaultValue={4}
								value={p.ratio}
								modColor={modColorFor(effect.id, 'ratio')}
								automationColor={automationColorFor(effect.id, 'ratio')}
								automatedValue={automatedValueFor(effect.id, 'ratio')}
								modTargetKey={`${effect.id}/ratio`}
								on:release={(e) => handleParam(effect.id, { ...p, ratio: e.detail })}
							/>
							<span class="val">{p.ratio.toFixed(1)}:1</span>
							<span class="lbl">ATK</span>
							<Knob
								min={0.1}
								max={200}
								defaultValue={10}
								value={p.attack_ms}
								modColor={modColorFor(effect.id, 'attack_ms')}
								automationColor={automationColorFor(effect.id, 'attack_ms')}
								automatedValue={automatedValueFor(effect.id, 'attack_ms')}
								modTargetKey={`${effect.id}/attack_ms`}
								on:release={(e) => handleParam(effect.id, { ...p, attack_ms: e.detail })}
							/>
							<span class="val">{p.attack_ms.toFixed(0)} ms</span>
							<span class="lbl">REL</span>
							<Knob
								min={10}
								max={2000}
								defaultValue={100}
								value={p.release_ms}
								modColor={modColorFor(effect.id, 'release_ms')}
								automationColor={automationColorFor(effect.id, 'release_ms')}
								automatedValue={automatedValueFor(effect.id, 'release_ms')}
								modTargetKey={`${effect.id}/release_ms`}
								on:release={(e) => handleParam(effect.id, { ...p, release_ms: e.detail })}
							/>
							<span class="val">{p.release_ms.toFixed(0)} ms</span>
							<span class="lbl">MKUP</span>
							<Knob
								min={-6}
								max={24}
								defaultValue={0}
								value={p.makeup_db}
								modColor={modColorFor(effect.id, 'makeup_db')}
								automationColor={automationColorFor(effect.id, 'makeup_db')}
								automatedValue={automatedValueFor(effect.id, 'makeup_db')}
								modTargetKey={`${effect.id}/makeup_db`}
								on:release={(e) => handleParam(effect.id, { ...p, makeup_db: e.detail })}
							/>
							<span class="val">{p.makeup_db.toFixed(1)} dB</span>
						</div>
					{:else if effect.params.kind === 'reverb'}
						{@const p = revP(effect)}
						<div class="grid">
							<span class="lbl">SIZE</span>
							<Knob
								min={0}
								max={1}
								defaultValue={0.5}
								value={p.room_size}
								modColor={modColorFor(effect.id, 'room_size')}
								automationColor={automationColorFor(effect.id, 'room_size')}
								automatedValue={automatedValueFor(effect.id, 'room_size')}
								modTargetKey={`${effect.id}/room_size`}
								on:release={(e) => handleParam(effect.id, { ...p, room_size: e.detail })}
							/>
							<span class="val">{(p.room_size * 100).toFixed(0)}%</span>
							<span class="lbl">DAMP</span>
							<Knob
								min={0}
								max={1}
								defaultValue={0.5}
								value={p.damping}
								modColor={modColorFor(effect.id, 'damping')}
								automationColor={automationColorFor(effect.id, 'damping')}
								automatedValue={automatedValueFor(effect.id, 'damping')}
								modTargetKey={`${effect.id}/damping`}
								on:release={(e) => handleParam(effect.id, { ...p, damping: e.detail })}
							/>
							<span class="val">{(p.damping * 100).toFixed(0)}%</span>
							<span class="lbl">WET</span>
							<Knob
								min={0}
								max={1}
								defaultValue={0.33}
								value={p.wet}
								modColor={modColorFor(effect.id, 'wet')}
								automationColor={automationColorFor(effect.id, 'wet')}
								automatedValue={automatedValueFor(effect.id, 'wet')}
								modTargetKey={`${effect.id}/wet`}
								on:release={(e) => handleParam(effect.id, { ...p, wet: e.detail })}
							/>
							<span class="val">{(p.wet * 100).toFixed(0)}%</span>
							<span class="lbl">WIDTH</span>
							<Knob
								min={0}
								max={1}
								defaultValue={1}
								value={p.width}
								modColor={modColorFor(effect.id, 'width')}
								automationColor={automationColorFor(effect.id, 'width')}
								automatedValue={automatedValueFor(effect.id, 'width')}
								modTargetKey={`${effect.id}/width`}
								on:release={(e) => handleParam(effect.id, { ...p, width: e.detail })}
							/>
							<span class="val">{(p.width * 100).toFixed(0)}%</span>
						</div>
					{:else if effect.params.kind === 'delay'}
						{@const p = delP(effect)}
						<div class="grid">
							<span class="lbl">TIME</span>
							<Knob
								min={10}
								max={2000}
								defaultValue={250}
								value={p.delay_ms}
								modColor={modColorFor(effect.id, 'delay_ms')}
								automationColor={automationColorFor(effect.id, 'delay_ms')}
								automatedValue={automatedValueFor(effect.id, 'delay_ms')}
								modTargetKey={`${effect.id}/delay_ms`}
								on:release={(e) => handleParam(effect.id, { ...p, delay_ms: e.detail })}
							/>
							<span class="val">{p.delay_ms.toFixed(0)} ms</span>
							<span class="lbl">FDBK</span>
							<Knob
								min={0}
								max={0.95}
								defaultValue={0.4}
								value={p.feedback}
								modColor={modColorFor(effect.id, 'feedback')}
								automationColor={automationColorFor(effect.id, 'feedback')}
								automatedValue={automatedValueFor(effect.id, 'feedback')}
								modTargetKey={`${effect.id}/feedback`}
								on:release={(e) => handleParam(effect.id, { ...p, feedback: e.detail })}
							/>
							<span class="val">{(p.feedback * 100).toFixed(0)}%</span>
							<span class="lbl">WET</span>
							<Knob
								min={0}
								max={1}
								defaultValue={0.3}
								value={p.wet}
								modColor={modColorFor(effect.id, 'wet')}
								automationColor={automationColorFor(effect.id, 'wet')}
								automatedValue={automatedValueFor(effect.id, 'wet')}
								modTargetKey={`${effect.id}/wet`}
								on:release={(e) => handleParam(effect.id, { ...p, wet: e.detail })}
							/>
							<span class="val">{(p.wet * 100).toFixed(0)}%</span>
							<span class="lbl">PING</span>
							<button
								class="toggle"
								class:on={p.ping_pong}
								type="button"
								on:click={() => handleParam(effect.id, { ...p, ping_pong: !p.ping_pong })}
							>
								{p.ping_pong ? 'ON' : 'OFF'}
							</button>
							<span class="val"></span>
						</div>
					{:else if unavailable}
						<div class="missing-row">
							{#if !compact}
								<p class="missing-help">{UNAVAILABLE_HELP[unavailable.reason]}</p>
							{/if}
							{#if unavailable.reason !== 'unsupported_format' && unavailable.reason !== 'crashed'}
								<button class="retry-btn" type="button" on:click={handleRetry} disabled={retrying}>
									{retrying ? 'Recherche…' : 'Réessayer'}
								</button>
							{/if}
						</div>
					{:else if effect.params.kind === 'plugin'}
						<div class="plugin-editor-row">
							<button
								class="open-editor-btn"
								class:active={openEditors.has(effect.id)}
								type="button"
								on:click={() => handleOpenEditor(effect)}
								title={openEditors.has(effect.id) ? "Fermer l'éditeur" : "Ouvrir l'éditeur"}
							>
								{compact
									? openEditors.has(effect.id)
										? '× GUI'
										: 'GUI'
									: openEditors.has(effect.id)
										? "Fermer l'éditeur"
										: "Ouvrir l'éditeur"}
							</button>
							{#if canBeIsolated(effect) && isolation === 'chosen'}
								{@const always = isAlwaysIsolated(effect, alwaysIsolated)}
								<button
									class="open-editor-btn"
									class:active={always || isChosen(effect)}
									type="button"
									disabled={always}
									on:click={() => handleToggleSandbox(effect)}
									title={always
										? "Ce plugin est toujours isolé (clic droit sur l'effet pour changer)"
										: "Faire tourner cet effet dans un processus séparé : s'il plante, Echoes continue (clic droit : toujours isoler ce plugin)"}
								>
									{compact ? 'ISO' : always || isChosen(effect) ? 'Isolé' : 'Isoler'}
								</button>
							{/if}
						</div>
						{#if effectFormat(effect) === 'CLAP' || effectFormat(effect) === 'VST3'}
							<PresetPicker instanceKey={effect.id} />
						{/if}
						{#if hasSidechainInput(effect, $plugins)}
							<SidechainPicker
								{trackId}
								effectId={effect.id}
								params={effect.params.params}
								disabledReason={effectFormat(effect) === 'CLAP' &&
								isSandboxed(effect, isolation, alwaysIsolated)
									? 'Le sidechain ne fonctionne pas quand le plugin CLAP est isolé'
									: null}
							/>
						{/if}
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
		&.compact .slot-head {
			padding: 2px 5px;
		}
		&.compact .slot-params {
			padding: 4px 5px 5px;
		}
		&.compact .empty {
			padding: 6px 5px;
			font-size: 9px;
		}
		&.compact .grid {
			grid-template-columns: 28px 26px 1fr;
			row-gap: 2px;
		}
		&.compact .lbl {
			font-size: 7px;
		}
		&.compact .val {
			font-size: 8px;
		}
		&.compact .slot-name {
			font-size: 9px;
		}
		&.compact .open-editor-btn {
			padding: 3px 8px;
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
		background: #222229;
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
		border-radius: 3px;
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
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;
			border-color: rgba(21, 113, 255, 0.4);
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

	.slot-head {
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 4px 6px;
	}

	.slot-toggle {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 5px;
		min-width: 0;
		padding: 0;
		border: none;
		background: transparent;
		color: inherit;
		font: inherit;
		cursor: pointer;
		text-align: left;
		&:hover {
			background: $surface-1;
			border-radius: 3px;
		}
	}

	.bypass {
		width: 13px;
		height: 13px;
		border-radius: 50%;
		border: 1px solid $border-subtle;
		background: transparent;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 0;
		flex-shrink: 0;
		color: $text-faint;
		&.on {
			color: $accent-color;
			border-color: $accent-color;
		}
	}

	.slot-name {
		flex: 1;
		font-size: 10px;
		font-weight: 500;
		color: $text-color;
		user-select: none;
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

	.slot.unavailable .slot-name {
		color: $text-faint;
		text-decoration: line-through;
	}

	.slot.crashed {
		border-left: 2px solid #ff4136;
		background: rgba(255, 65, 54, 0.08);
		opacity: 1;
		.slot-name {
			color: #ff8a82;
			text-decoration: none;
		}
	}

	.reload-btn {
		flex-shrink: 0;
		padding: 2px 8px;
		border: 1px solid rgba(255, 65, 54, 0.6);
		border-radius: 3px;
		background: rgba(255, 65, 54, 0.18);
		color: #ffb3ad;
		font-size: 9px;
		font-family: $font-mono;
		font-weight: 700;
		cursor: pointer;
		&:hover:not(:disabled) {
			background: rgba(255, 65, 54, 0.32);
			color: #fff;
		}
		&:disabled {
			cursor: default;
			opacity: 0.6;
		}
	}

	.missing-badge {
		font-size: 7px;
		font-family: $font-mono;
		font-weight: 700;
		padding: 1px 4px;
		border-radius: 2px;
		flex-shrink: 0;
		white-space: nowrap;
		border: 1px solid rgba(255, 65, 54, 0.5);
		background: rgba(255, 65, 54, 0.12);
		color: #ff6b61;
	}

	.missing-row {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 6px;
		padding: 4px 2px;
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

	.chevron {
		flex-shrink: 0;
		color: $text-faint;
		transition: transform 0.15s;
		&.open {
			transform: rotate(180deg);
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
		display: flex;
		align-items: center;
		justify-content: center;
		flex-shrink: 0;
		&:hover {
			color: #ff4136;
		}
	}

	.slot-params {
		padding: 6px 8px 8px;
		background: rgba(0, 0, 0, 0.15);
	}

	.plugin-editor-row {
		display: flex;
		justify-content: center;
		gap: 6px;
		padding: 8px 4px;
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
		transition:
			background 0.1s,
			color 0.1s,
			border-color 0.1s;
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

	.grid {
		display: grid;
		grid-template-columns: 36px 28px 1fr;
		row-gap: 4px;
		column-gap: 4px;
		align-items: center;
	}

	.lbl {
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		color: $text-faint;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		text-align: right;
		user-select: none;
	}

	.val {
		font-size: 9px;
		font-family: $font-mono;
		color: $text-subtle;
		padding-left: 2px;
	}

	.toggle {
		height: 18px;
		padding: 0 5px;
		border: 1px solid $border-subtle;
		border-radius: 3px;
		background: transparent;
		color: $text-subtle;
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		cursor: pointer;
		&.on {
			background: rgba(21, 113, 255, 0.2);
			border-color: rgba(21, 113, 255, 0.5);
			color: $accent-color;
		}
	}
</style>
