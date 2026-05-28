<!-- ModulationPanel.svelte — LFO sources and modulation routes per track -->
<script lang="ts">
	import Knob from './Knob.svelte';
	import type { TrackEffect, ProjectModulator, ProjectModulationRoute } from '../types/global';
	import {
		addModulator,
		removeModulator,
		updateModulator,
		removeModulationRoute,
		setModulationDepth,
	} from '../frontend/commands/dispatcher';
	import { notify } from '../stores/status';

	export let trackId: string;
	export let modulators: ProjectModulator[];
	export let mod_routes: ProjectModulationRoute[];
	export let effects: TrackEffect[];

	const EFFECT_LABELS: Record<string, string> = {
		eq: 'EQ', compressor: 'Comp', reverb: 'Réverb', delay: 'Délai',
	};

	const PARAM_LABELS: Record<string, string> = {
		gain: 'Vol',
		low_gain_db: 'Low', mid_gain_db: 'Mid', high_gain_db: 'High', mid_q: 'Q',
		threshold_db: 'Seuil', ratio: 'Ratio', attack_ms: 'Attq', release_ms: 'Rel', makeup_db: 'Mkup',
		room_size: 'Taille', damping: 'Damp', wet: 'Wet', width: 'Width',
		delay_ms: 'Time', feedback: 'Fdbk',
	};

	function routeTargetLabel(route: ProjectModulationRoute): string {
		if (route.target.kind === 'track_gain') return 'Volume';
		const t = route.target as { kind: 'effect_param'; track_id: string; effect_id: string; param: string };
		const effect = effects.find((e) => e.id === t.effect_id);
		const effectLabel = effect ? (EFFECT_LABELS[effect.params.kind] ?? effect.params.kind) : '?';
		return `${effectLabel} › ${PARAM_LABELS[t.param] ?? t.param}`;
	}

	function modNameForRoute(route: ProjectModulationRoute): string {
		const idx = modulators.findIndex((m) => m.id === route.modulator_id);
		return idx >= 0 ? `LFO ${idx + 1}` : '?';
	}

	async function handleAddLfo() {
		try { await addModulator(trackId, 'lfo', 1.0); }
		catch (e) { notify(`Erreur ajout modulateur : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	async function handleRemoveMod(modId: string) {
		try { await removeModulator(trackId, modId); }
		catch (e) { notify(`Erreur suppression : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	async function handleRateChange(modId: string, rate_hz: number) {
		try { await updateModulator(trackId, modId, { rate_hz }); }
		catch (e) { notify(`Erreur taux : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	async function handleRemoveRoute(routeId: string) {
		try { await removeModulationRoute(trackId, routeId); }
		catch (e) { notify(`Erreur suppression route : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	async function handleDepthChange(routeId: string, e: Event) {
		const value = (e.currentTarget as HTMLInputElement).value;
		try { await setModulationDepth(trackId, routeId, parseFloat(value)); }
		catch (err) { notify(`Erreur profondeur : ${(err as Error)?.message ?? err}`, 'error'); }
	}
</script>

<div class="mod-panel">

	<!-- ── Sources ─────────────────────────────────────────────────────────────── -->
	<div class="section-header">
		<span class="section-title">Modulateurs</span>
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<button class="add-btn" type="button" on:click={handleAddLfo} title="Ajouter LFO">+</button>
	</div>

	{#if modulators.length === 0}
		<p class="empty">Aucun modulateur — cliquez + pour en ajouter</p>
	{/if}

	{#each modulators as mod, i (mod.id)}
		<div class="slot">
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<div class="slot-head">
				<div class="lfo-icon" aria-hidden="true">
					<svg width="16" height="10" viewBox="0 0 16 10" fill="none">
						<path d="M0 5 Q2 0 4 5 Q6 10 8 5 Q10 0 12 5 Q14 10 16 5"
							stroke="currentColor" stroke-width="1.2" fill="none" stroke-linecap="round"/>
					</svg>
				</div>
				<span class="slot-name">LFO {i + 1}</span>
				<span class="param-lbl">RATE</span>
				<Knob
					min={0.1}
					max={20}
					defaultValue={1}
					value={mod.rate_hz ?? 1}
					on:release={(e) => handleRateChange(mod.id, e.detail)}
				/>
				<span class="param-val">{(mod.rate_hz ?? 1).toFixed(2)} Hz</span>
				<button class="rm-btn" type="button"
					on:click|stopPropagation={() => handleRemoveMod(mod.id)} title="Supprimer">×</button>
			</div>
		</div>
	{/each}

	<!-- ── Routes ──────────────────────────────────────────────────────────────── -->
	<div class="section-header routes-header">
		<span class="section-title">Routes</span>
	</div>

	{#if mod_routes.length === 0}
		<p class="empty">Aucune route — assignez un LFO à un paramètre</p>
	{/if}

	{#each mod_routes as route (route.id)}
		<div class="slot">
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<div class="route-row">
				<span class="route-src">{modNameForRoute(route)}</span>
				<span class="route-arrow">→</span>
				<span class="route-tgt">{routeTargetLabel(route)}</span>
				<input
					class="depth-slider"
					type="range"
					min="-1"
					max="1"
					step="0.01"
					value={route.depth}
					on:change={(e) => handleDepthChange(route.id, e)}
				/>
				<span class="depth-val">{route.depth >= 0 ? '+' : ''}{(route.depth * 100).toFixed(0)}%</span>
				<button class="rm-btn" type="button"
					on:click|stopPropagation={() => handleRemoveRoute(route.id)} title="Supprimer">×</button>
			</div>
		</div>
	{/each}

</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.mod-panel { width: 100%; }

	.section-header {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 5px 6px;
		border-bottom: 1px solid $border-faint;
		background: #222229;
		position: sticky;
		top: 0;
		z-index: 1;

		&.routes-header {
			position: static;
			margin-top: 2px;
		}
	}

	.section-title {
		flex: 1;
		font-size: 10px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.08em;
		color: $text-muted;
		text-transform: uppercase;
	}

	.add-btn {
		width: 18px; height: 18px;
		border: 1px solid $border-subtle; border-radius: 3px;
		background: transparent; color: $text-subtle;
		font-size: 14px; line-height: 1; cursor: pointer;
		display: flex; align-items: center; justify-content: center; padding: 0;
		&:hover { background: rgba(21,113,255,0.15); color: $text-color; border-color: rgba(21,113,255,0.4); }
	}

	.empty {
		font-size: 10px; color: $text-faint; text-align: center;
		padding: 12px 8px; margin: 0;
	}

	.slot { border-bottom: 1px solid $border-faint; }

	.slot-head {
		display: flex; align-items: center; gap: 5px;
		padding: 4px 6px;
		&:hover { background: $surface-1; }
	}

	.lfo-icon {
		flex-shrink: 0;
		width: 20px;
		display: flex; align-items: center;
		color: rgba(21,113,255,0.75);
	}

	.slot-name {
		flex-shrink: 0;
		min-width: 38px;
		font-size: 10px; font-weight: 500; color: $text-color;
		user-select: none; white-space: nowrap;
	}

	.param-lbl {
		font-size: 8px; font-family: $font-mono; font-weight: 700;
		color: $text-faint; letter-spacing: 0.05em; text-transform: uppercase;
		flex-shrink: 0;
	}

	.param-val {
		font-size: 9px; font-family: $font-mono; color: $text-subtle;
		flex-shrink: 0; min-width: 44px;
	}

	.rm-btn {
		margin-left: auto;
		width: 16px; height: 16px; border: none; background: transparent;
		color: $text-faint; font-size: 14px; line-height: 1; cursor: pointer;
		padding: 0; display: flex; align-items: center; justify-content: center;
		flex-shrink: 0;
		&:hover { color: #ff4136; }
	}

	// Routes
	.route-row {
		display: flex; align-items: center; gap: 5px;
		padding: 5px 6px;
		&:hover { background: $surface-1; }
	}

	.route-src {
		flex-shrink: 0;
		min-width: 36px;
		font-size: 9px; font-family: $font-mono; font-weight: 700;
		color: rgba(21,113,255,0.9);
	}

	.route-arrow {
		flex-shrink: 0;
		font-size: 10px; color: $text-faint;
	}

	.route-tgt {
		flex: 1;
		font-size: 9px; font-family: $font-mono; color: $text-subtle;
		overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
		min-width: 0;
	}

	.depth-slider {
		flex-shrink: 0;
		width: 52px; height: 3px;
		cursor: pointer;
		accent-color: rgba(21,113,255,0.85);
	}

	.depth-val {
		flex-shrink: 0;
		min-width: 32px;
		text-align: right;
		font-size: 9px; font-family: $font-mono; color: $text-subtle;
	}
</style>
