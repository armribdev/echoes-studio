<!-- EffectChain.svelte — per-track insert effects panel (built-in + VST3/CLAP) -->
<script lang="ts">
	import Knob from './Knob.svelte';
	import PluginPicker from './PluginPicker.svelte';
	import type { TrackEffect, EqParams, CompressorParams, ReverbParams, DelayParams } from '../types/global';
	import type { PluginInfo } from '../stores/plugins';
	import { loadPlugins, plugins } from '../stores/plugins';
	import { addEffect, removeEffect, updateEffect } from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';
	import { invoke } from '@tauri-apps/api/core';

	export let trackId: string;
	export let effects: TrackEffect[];
	export let compact = false;

	let expanded:    Record<string, boolean> = {};
	let showAddMenu  = false;
	let showPicker   = false;
	// effect_ids that currently have an open native editor window
	let openEditors: Set<string> = new Set();

	// Ensure plugins are loaded (no-op if already loaded)
	if ($plugins.length === 0) loadPlugins();

	function toggleExpand(id: string) {
		expanded[id] = !expanded[id];
		expanded = expanded;
	}

	// ── Built-in effects ───────────────────────────────────────────────────────

	async function handleAddBuiltin(kind: string) {
		showAddMenu = false;
		try { await addEffect(trackId, kind); }
		catch (e) { notify(`Erreur ajout effet : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	// ── Plugin effects (VST3 / CLAP) ──────────────────────────────────────────

	async function handlePickPlugin(p: PluginInfo) {
		showPicker   = false;
		showAddMenu  = false;
		// Encode name + format into the kind string so the backend can cache them.
		const kind = `plugin:${p.id}|${p.name}|${p.format}`;
		try { await addEffect(trackId, kind); }
		catch (e) { notify(`Erreur ajout plugin : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	async function handleRemoveEffect(effectId: string) {
		try { await removeEffect(trackId, effectId); }
		catch (e) { notify(`Erreur suppression : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	async function handleToggle(effect: TrackEffect) {
		try { await updateEffect(trackId, effect.id, { enabled: !effect.enabled }); }
		catch (e) { notify(`Erreur bypass : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	async function handleParam(effectId: string, params: Record<string, unknown>) {
		try { await updateEffect(trackId, effectId, { params }); }
		catch (e) { notify(`Erreur paramètre : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	async function handleOpenEditor(effect: TrackEffect) {
		// EffectParams::Plugin serialises as { kind: "plugin", params: { plugin_id, name, … } }
		const p = (effect.params as any).params ?? {};
		const isOpen = openEditors.has(effect.id);
		try {
			if (isOpen) {
				await invoke('close_plugin_editor', { effectId: effect.id });
				openEditors.delete(effect.id);
			} else {
				await invoke('open_plugin_editor', {
					pluginId:   p.plugin_id,
					effectId:   effect.id,
					pluginName: p.name ?? 'Plugin',
				});
				openEditors.add(effect.id);
			}
			openEditors = new Set(openEditors); // trigger reactivity
		} catch (e) {
			notify(`Erreur éditeur plugin : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	const BUILTIN_LABELS: Record<string, string> = {
		eq: 'EQ 3-bandes', compressor: 'Compresseur', reverb: 'Réverb', delay: 'Délai',
	};

	function effectLabel(effect: TrackEffect): string {
		if (effect.params.kind === 'plugin') return (effect.params as any).params?.name ?? 'Plugin';
		return BUILTIN_LABELS[effect.params.kind] ?? effect.params.kind;
	}

	function effectFormat(effect: TrackEffect): string | null {
		if (effect.params.kind === 'plugin') return (effect.params as any).params?.format ?? null;
		return null;
	}

	const FORMAT_COLOR: Record<string, string> = {
		VST3: '#1571ff',
		CLAP: '#22c55e',
		AU:   '#f59e0b',
	};

	function eqP(e: TrackEffect) { return (e.params as { kind: 'eq'; params: EqParams }).params; }
	function compP(e: TrackEffect) { return (e.params as { kind: 'compressor'; params: CompressorParams }).params; }
	function revP(e: TrackEffect) { return (e.params as { kind: 'reverb'; params: ReverbParams }).params; }
	function delP(e: TrackEffect) { return (e.params as { kind: 'delay'; params: DelayParams }).params; }
</script>

{#if showPicker}
	<PluginPicker
		mode="effect"
		on:pick={(e) => handlePickPlugin(e.detail)}
		on:cancel={() => (showPicker = false)}
	/>
{/if}

<div class="fx-panel" class:compact>
	<div class="fx-header" style:z-index={showAddMenu ? 10 : 1}>
		<span class="fx-title">{compact ? 'FX' : 'Effets'}</span>
		<div class="add-wrap">
			<button class="add-btn" type="button" on:click|stopPropagation={() => (showAddMenu = !showAddMenu)} title="Ajouter">+</button>
			{#if showAddMenu}
				<div class="add-menu" on:pointerdown|stopPropagation>
					{#each Object.entries(BUILTIN_LABELS) as [kind, label]}
						<button class="menu-item" type="button" on:click={() => handleAddBuiltin(kind)}>{label}</button>
					{/each}
					<div class="menu-separator"></div>
					<button class="menu-item menu-item--plugin" type="button" on:click={() => { showAddMenu = false; showPicker = true; }}>
						<span>VST3 / CLAP…</span>
						<span class="menu-badges">
							<span class="badge" style:background="#1571ff">VST3</span>
							<span class="badge" style:background="#22c55e">CLAP</span>
						</span>
					</button>
				</div>
			{/if}
		</div>
	</div>

	{#if effects.length === 0}
		<p class="empty">{compact ? '—' : 'Aucun effet — cliquez + pour en ajouter'}</p>
	{/if}

	{#each effects as effect (effect.id)}
		<div class="slot" class:bypassed={!effect.enabled}>
			<div class="slot-head">
				<button class="bypass" class:on={effect.enabled} type="button"
					on:click|stopPropagation={() => handleToggle(effect)} title="Bypass">
					<svg width="8" height="8" viewBox="0 0 8 8"><circle cx="4" cy="4" r="3" fill="currentColor"/></svg>
				</button>
				<button class="slot-toggle" type="button" on:click={() => toggleExpand(effect.id)}>
					<span class="slot-name">{effectLabel(effect)}</span>
					{#if effectFormat(effect)}
						<span class="fmt-badge" style:background={FORMAT_COLOR[effectFormat(effect) ?? ''] ?? '#555'}>
							{effectFormat(effect)}
						</span>
					{/if}
					<svg class="chevron" class:open={expanded[effect.id]} width="8" height="5" viewBox="0 0 8 5">
						<path d="M1 1 L4 4 L7 1" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round"/>
					</svg>
				</button>
				<button class="rm-btn" type="button" on:click|stopPropagation={() => handleRemoveEffect(effect.id)} title="Supprimer">×</button>
			</div>

			{#if expanded[effect.id]}
				<div class="slot-params">
					{#if effect.params.kind === 'eq'}
						{@const p = eqP(effect)}
						<div class="grid">
							<span class="lbl">LOW</span>
							<Knob min={-12} max={12} defaultValue={0} value={p.low_gain_db} on:release={(e) => handleParam(effect.id, { ...p, low_gain_db: e.detail })}/>
							<span class="val">{p.low_gain_db.toFixed(1)} dB</span>
							<span class="lbl">MID</span>
							<Knob min={-12} max={12} defaultValue={0} value={p.mid_gain_db} on:release={(e) => handleParam(effect.id, { ...p, mid_gain_db: e.detail })}/>
							<span class="val">{p.mid_gain_db.toFixed(1)} dB</span>
							<span class="lbl">HIGH</span>
							<Knob min={-12} max={12} defaultValue={0} value={p.high_gain_db} on:release={(e) => handleParam(effect.id, { ...p, high_gain_db: e.detail })}/>
							<span class="val">{p.high_gain_db.toFixed(1)} dB</span>
							<span class="lbl">Q</span>
							<Knob min={0.1} max={10} defaultValue={1} value={p.mid_q} on:release={(e) => handleParam(effect.id, { ...p, mid_q: e.detail })}/>
							<span class="val">{p.mid_q.toFixed(2)}</span>
						</div>

					{:else if effect.params.kind === 'compressor'}
						{@const p = compP(effect)}
						<div class="grid">
							<span class="lbl">THR</span>
							<Knob min={-60} max={0} defaultValue={-18} value={p.threshold_db} on:release={(e) => handleParam(effect.id, { ...p, threshold_db: e.detail })}/>
							<span class="val">{p.threshold_db.toFixed(1)} dB</span>
							<span class="lbl">RATIO</span>
							<Knob min={1} max={20} defaultValue={4} value={p.ratio} on:release={(e) => handleParam(effect.id, { ...p, ratio: e.detail })}/>
							<span class="val">{p.ratio.toFixed(1)}:1</span>
							<span class="lbl">ATK</span>
							<Knob min={0.1} max={200} defaultValue={10} value={p.attack_ms} on:release={(e) => handleParam(effect.id, { ...p, attack_ms: e.detail })}/>
							<span class="val">{p.attack_ms.toFixed(0)} ms</span>
							<span class="lbl">REL</span>
							<Knob min={10} max={2000} defaultValue={100} value={p.release_ms} on:release={(e) => handleParam(effect.id, { ...p, release_ms: e.detail })}/>
							<span class="val">{p.release_ms.toFixed(0)} ms</span>
							<span class="lbl">MKUP</span>
							<Knob min={-6} max={24} defaultValue={0} value={p.makeup_db} on:release={(e) => handleParam(effect.id, { ...p, makeup_db: e.detail })}/>
							<span class="val">{p.makeup_db.toFixed(1)} dB</span>
						</div>

					{:else if effect.params.kind === 'reverb'}
						{@const p = revP(effect)}
						<div class="grid">
							<span class="lbl">SIZE</span>
							<Knob min={0} max={1} defaultValue={0.5} value={p.room_size} on:release={(e) => handleParam(effect.id, { ...p, room_size: e.detail })}/>
							<span class="val">{(p.room_size*100).toFixed(0)}%</span>
							<span class="lbl">DAMP</span>
							<Knob min={0} max={1} defaultValue={0.5} value={p.damping} on:release={(e) => handleParam(effect.id, { ...p, damping: e.detail })}/>
							<span class="val">{(p.damping*100).toFixed(0)}%</span>
							<span class="lbl">WET</span>
							<Knob min={0} max={1} defaultValue={0.33} value={p.wet} on:release={(e) => handleParam(effect.id, { ...p, wet: e.detail })}/>
							<span class="val">{(p.wet*100).toFixed(0)}%</span>
							<span class="lbl">WIDTH</span>
							<Knob min={0} max={1} defaultValue={1} value={p.width} on:release={(e) => handleParam(effect.id, { ...p, width: e.detail })}/>
							<span class="val">{(p.width*100).toFixed(0)}%</span>
						</div>

					{:else if effect.params.kind === 'delay'}
						{@const p = delP(effect)}
						<div class="grid">
							<span class="lbl">TIME</span>
							<Knob min={10} max={2000} defaultValue={250} value={p.delay_ms} on:release={(e) => handleParam(effect.id, { ...p, delay_ms: e.detail })}/>
							<span class="val">{p.delay_ms.toFixed(0)} ms</span>
							<span class="lbl">FDBK</span>
							<Knob min={0} max={0.95} defaultValue={0.4} value={p.feedback} on:release={(e) => handleParam(effect.id, { ...p, feedback: e.detail })}/>
							<span class="val">{(p.feedback*100).toFixed(0)}%</span>
							<span class="lbl">WET</span>
							<Knob min={0} max={1} defaultValue={0.3} value={p.wet} on:release={(e) => handleParam(effect.id, { ...p, wet: e.detail })}/>
							<span class="val">{(p.wet*100).toFixed(0)}%</span>
							<span class="lbl">PING</span>
							<button class="toggle" class:on={p.ping_pong} type="button"
								on:click={() => handleParam(effect.id, { ...p, ping_pong: !p.ping_pong })}>
								{p.ping_pong ? 'ON' : 'OFF'}
							</button>
							<span class="val"></span>
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
									? (openEditors.has(effect.id) ? '× GUI' : 'GUI')
									: openEditors.has(effect.id)
										? "Fermer l'éditeur"
										: "Ouvrir l'éditeur"}
							</button>
						</div>
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

		&.compact .fx-header { padding: 3px 5px; }
		&.compact .slot-head { padding: 2px 5px; }
		&.compact .slot-params { padding: 4px 5px 5px; }
		&.compact .empty { padding: 6px 5px; font-size: 9px; }
		&.compact .grid {
			grid-template-columns: 28px 26px 1fr;
			row-gap: 2px;
		}
		&.compact .lbl { font-size: 7px; }
		&.compact .val { font-size: 8px; }
		&.compact .slot-name { font-size: 9px; }
		&.compact .open-editor-btn {
			padding: 3px 8px;
			font-size: 9px;
		}
	}

	.fx-header {
		display: flex; align-items: center; gap: 4px;
		padding: 5px 6px;
		border-bottom: 1px solid $border-faint;
		position: sticky; top: 0;
		background: #222229;
		z-index: 1;
	}

	.fx-title {
		flex: 1; font-size: 10px; font-weight: 700;
		font-family: $font-mono; letter-spacing: 0.08em;
		color: $text-muted; text-transform: uppercase;
	}

	.add-wrap { position: relative; }

	.add-btn {
		width: 18px; height: 18px;
		border: 1px solid $border-subtle; border-radius: 3px;
		background: transparent; color: $text-subtle;
		font-size: 14px; line-height: 1; cursor: pointer;
		display: flex; align-items: center; justify-content: center; padding: 0;
		&:hover { background: rgba(21,113,255,0.15); color: $text-color; border-color: rgba(21,113,255,0.4); }
	}

	.add-menu {
		position: absolute; right: 0; top: calc(100% + 4px);
		background: #2a2a32; border: 1px solid $border-color;
		border-radius: $radius-md; z-index: 100; min-width: 150px;
		overflow: hidden; box-shadow: 0 4px 12px rgba(0,0,0,0.4);
	}

	.menu-item {
		display: flex; align-items: center; width: 100%; padding: 6px 10px;
		background: transparent; border: none; color: $text-color;
		font-size: 11px; font-family: inherit; text-align: left; cursor: pointer;
		gap: 6px;
		&:hover { background: $surface-hover; }
		&--plugin { justify-content: space-between; }
	}

	.menu-separator {
		height: 1px; background: $border-faint; margin: 3px 0;
	}

	.menu-badges { display: flex; gap: 3px; }

	.badge {
		font-size: 7px; font-family: $font-mono; font-weight: 700;
		letter-spacing: 0.04em; padding: 1px 4px; border-radius: 2px; color: #fff;
	}

	.empty {
		font-size: 10px; color: $text-faint; text-align: center;
		padding: 12px 8px; margin: 0;
	}

	.slot {
		border-bottom: 1px solid $border-faint;
		&.bypassed { opacity: 0.4; }
	}

	.slot-head {
		display: flex; align-items: center; gap: 5px;
		padding: 4px 6px;
	}

	.slot-toggle {
		flex: 1; display: flex; align-items: center; gap: 5px; min-width: 0;
		padding: 0; border: none; background: transparent;
		color: inherit; font: inherit; cursor: pointer; text-align: left;
		&:hover { background: $surface-1; border-radius: 3px; }
	}

	.bypass {
		width: 13px; height: 13px; border-radius: 50%;
		border: 1px solid $border-subtle; background: transparent;
		cursor: pointer; display: flex; align-items: center; justify-content: center;
		padding: 0; flex-shrink: 0; color: $text-faint;
		&.on { color: $accent-color; border-color: $accent-color; }
	}

	.slot-name {
		flex: 1; font-size: 10px; font-weight: 500; color: $text-color;
		user-select: none; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
	}

	.fmt-badge {
		font-size: 7px; font-family: $font-mono; font-weight: 700;
		letter-spacing: 0.04em; padding: 1px 4px; border-radius: 2px;
		color: #fff; flex-shrink: 0;
	}

	.chevron {
		flex-shrink: 0; color: $text-faint; transition: transform 0.15s;
		&.open { transform: rotate(180deg); }
	}

	.rm-btn {
		width: 16px; height: 16px; border: none; background: transparent;
		color: $text-faint; font-size: 14px; line-height: 1; cursor: pointer;
		padding: 0; display: flex; align-items: center; justify-content: center;
		flex-shrink: 0;
		&:hover { color: #ff4136; }
	}

	.slot-params {
		padding: 6px 8px 8px;
		background: rgba(0,0,0,0.15);
	}

	.plugin-editor-row {
		display: flex; justify-content: center; padding: 8px 4px;
	}

	.open-editor-btn {
		padding: 5px 14px;
		border: 1px solid $border-subtle; border-radius: 4px;
		background: transparent; color: $text-subtle;
		font-size: 10px; font-family: $font-mono; font-weight: 600;
		letter-spacing: 0.04em; cursor: pointer;
		transition: background 0.1s, color 0.1s, border-color 0.1s;
		&:hover { background: rgba(21,113,255,0.15); color: $text-color; border-color: rgba(21,113,255,0.4); }
		&.active { background: rgba(21,113,255,0.2); color: $accent-color; border-color: rgba(21,113,255,0.5); }
	}

	.grid {
		display: grid;
		grid-template-columns: 36px 28px 1fr;
		row-gap: 4px; column-gap: 4px;
		align-items: center;
	}

	.lbl {
		font-size: 8px; font-family: $font-mono; font-weight: 700;
		color: $text-faint; letter-spacing: 0.05em; text-transform: uppercase;
		text-align: right; user-select: none;
	}

	.val {
		font-size: 9px; font-family: $font-mono; color: $text-subtle; padding-left: 2px;
	}

	.toggle {
		height: 18px; padding: 0 5px;
		border: 1px solid $border-subtle; border-radius: 3px;
		background: transparent; color: $text-subtle;
		font-size: 8px; font-family: $font-mono; font-weight: 700; cursor: pointer;
		&.on { background: rgba(21,113,255,0.2); border-color: rgba(21,113,255,0.5); color: $accent-color; }
	}
</style>
