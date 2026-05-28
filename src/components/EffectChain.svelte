<!-- EffectChain.svelte — per-track insert effects panel (floating overlay) -->
<script lang="ts">
	import Knob from './Knob.svelte';
	import type { TrackEffect, EqParams, CompressorParams, ReverbParams, DelayParams } from '../types/global';
	import { addEffect, removeEffect, updateEffect } from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';

	export let trackId: string;
	export let effects: TrackEffect[];

	let expanded: Record<string, boolean> = {};
	let showAddMenu = false;

	function toggleExpand(id: string) {
		expanded[id] = !expanded[id];
		expanded = expanded;
	}

	async function handleAddEffect(kind: string) {
		showAddMenu = false;
		try { await addEffect(trackId, kind); }
		catch (e) { notify(`Erreur ajout effet : ${(e as Error)?.message ?? e}`, 'error'); }
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

	const LABELS: Record<string, string> = {
		eq: 'EQ 3-bandes', compressor: 'Compresseur', reverb: 'Réverb', delay: 'Délai',
	};

	function eqP(e: TrackEffect) { return (e.params as { kind: 'eq'; params: EqParams }).params; }
	function compP(e: TrackEffect) { return (e.params as { kind: 'compressor'; params: CompressorParams }).params; }
	function revP(e: TrackEffect) { return (e.params as { kind: 'reverb'; params: ReverbParams }).params; }
	function delP(e: TrackEffect) { return (e.params as { kind: 'delay'; params: DelayParams }).params; }
</script>

<div class="fx-panel">
	<div class="fx-header" style:z-index={showAddMenu ? 10 : 1}>
		<span class="fx-title">Effets</span>
		<div class="add-wrap">
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<button class="add-btn" type="button" on:click|stopPropagation={() => (showAddMenu = !showAddMenu)} title="Ajouter">+</button>
			{#if showAddMenu}
				<!-- svelte-ignore a11y-no-static-element-interactions -->
				<div class="add-menu" on:click|stopPropagation>
					{#each Object.entries(LABELS) as [kind, label]}
						<button class="menu-item" type="button" on:click={() => handleAddEffect(kind)}>{label}</button>
					{/each}
				</div>
			{/if}
		</div>
	</div>

	{#if effects.length === 0}
		<p class="empty">Aucun effet — cliquez + pour en ajouter</p>
	{/if}

	{#each effects as effect (effect.id)}
		<div class="slot" class:bypassed={!effect.enabled}>
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<div class="slot-head" on:click={() => toggleExpand(effect.id)}>
				<button class="bypass" class:on={effect.enabled} type="button"
					on:click|stopPropagation={() => handleToggle(effect)} title="Bypass">
					<svg width="8" height="8" viewBox="0 0 8 8"><circle cx="4" cy="4" r="3" fill="currentColor"/></svg>
				</button>
				<span class="slot-name">{LABELS[effect.params.kind] ?? effect.params.kind}</span>
				<svg class="chevron" class:open={expanded[effect.id]} width="8" height="5" viewBox="0 0 8 5">
					<path d="M1 1 L4 4 L7 1" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round"/>
				</svg>
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
		border-radius: $radius-md; z-index: 100; min-width: 130px;
		overflow: hidden; box-shadow: 0 4px 12px rgba(0,0,0,0.4);
	}

	.menu-item {
		display: block; width: 100%; padding: 6px 10px;
		background: transparent; border: none; color: $text-color;
		font-size: 11px; font-family: inherit; text-align: left; cursor: pointer;
		&:hover { background: $surface-hover; }
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
		padding: 4px 6px; cursor: pointer;
		&:hover { background: $surface-1; }
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
