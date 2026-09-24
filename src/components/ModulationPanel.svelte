<!-- ModulationPanel.svelte — modulators (LFO, envelope, follower, macro) and their routes per track -->
<script lang="ts">
	import LfoCard from './LfoCard.svelte';
	import EnvelopeCard from './EnvelopeCard.svelte';
	import FollowerCard from './FollowerCard.svelte';
	import MacroCard from './MacroCard.svelte';
	import ModulatorTypeMenu from './ModulatorTypeMenu.svelte';
	import ModulationTargetPicker from './ModulationTargetPicker.svelte';
	import { modulatorColor } from '../frontend/modulation/colors';
	import { modulatorLabel } from '../frontend/modulation/modulators';
	import type {
		ModulatorKind,
		ProjectModulationRoute,
		ProjectModulator,
		TrackEffect,
	} from '../types/global';
	import {
		addModulator,
		removeModulator,
		addModulationRoute,
		removeModulationRoute,
		setModulationDepth,
	} from '../frontend/commands/dispatcher';
	import {
		buildTargetOptions,
		fetchPluginParams,
		findTargetOption,
		pluginPlacements,
		type ModulationTargetOption,
		type PluginParamInfo,
		type PluginPlacement,
	} from '../frontend/modulation/targets';
	import { plugins, pluginDisplayName } from '../stores/plugins';
	import { notify } from '../stores/status';

	export let trackId: string;
	export let instrument: string | null = null;
	export let modulators: ProjectModulator[];
	export let mod_routes: ProjectModulationRoute[];
	export let effects: TrackEffect[];
	export let compact = false;

	const DEFAULT_DEPTH = 0.5;

	let pickerModulatorId: string | null = null;
	let typeMenuOpen = false;
	let paramsByPlacement: Record<string, PluginParamInfo[]> = {};
	let loadingParams = false;
	let paramsRequest = 0;

	$: instrumentName = instrument
		? ($plugins.find((p) => p.id === instrument)?.name ?? pluginDisplayName(instrument))
		: null;
	$: placements = pluginPlacements(trackId, effects, instrumentName);
	$: placementSignature = placements.map((p) => `${p.key}=${p.name}`).join('|');
	$: placementSignature, loadPluginParams(placements);
	$: options = buildTargetOptions(effects, placements, paramsByPlacement);
	$: pickerOptions = options.filter((o) => o.listed);

	async function loadPluginParams(list: PluginPlacement[]) {
		const request = ++paramsRequest;
		loadingParams = true;
		const entries = await Promise.all(list.map(async (p) => [p.key, await fetchPluginParams(p.key)] as const));
		if (request !== paramsRequest) return;
		paramsByPlacement = Object.fromEntries(entries);
		loadingParams = false;
	}

	function routeTargetLabel(route: ProjectModulationRoute, list: ModulationTargetOption[]): string {
		const option = findTargetOption(list, route.target);
		if (option) return option.kind === 'track_gain' ? option.label : `${option.group} › ${option.label}`;
		if (route.target.kind === 'plugin_param') return `Plugin › réglage ${route.target.native_id}`;
		return route.target.kind === 'effect_param' ? `Effet › ${route.target.param}` : 'Volume';
	}

	function depthScale(route: ProjectModulationRoute, list: ModulationTargetOption[]): number {
		return findTargetOption(list, route.target)?.depthScale ?? 1;
	}

	function sliderDepth(route: ProjectModulationRoute, list: ModulationTargetOption[]): number {
		return Math.max(-1, Math.min(1, route.depth / depthScale(route, list)));
	}

	function formatDepth(slider: number): string {
		return `${slider >= 0 ? '+' : ''}${(slider * 100).toFixed(0)}${compact ? '' : '%'}`;
	}

	function togglePicker(modId: string) {
		pickerModulatorId = pickerModulatorId === modId ? null : modId;
		if (pickerModulatorId) loadPluginParams(placements);
	}

	async function handleAssign(modId: string, option: ModulationTargetOption) {
		pickerModulatorId = null;
		try {
			await addModulationRoute(trackId, modId, option.kind, option.param, DEFAULT_DEPTH * option.depthScale, option.effectId);
		} catch (e) {
			notify(`Erreur assignation : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	function modIndexForRoute(route: ProjectModulationRoute): number {
		return modulators.findIndex((m) => m.id === route.modulator_id);
	}

	function modNameForRoute(route: ProjectModulationRoute): string {
		const mod = modulators[modIndexForRoute(route)];
		return mod ? modulatorLabel(modulators, mod) : '?';
	}

	function colorForRoute(route: ProjectModulationRoute): string {
		const idx = modIndexForRoute(route);
		return modulatorColor(idx >= 0 ? idx : 0);
	}

	async function handleAddModulator(kind: ModulatorKind) {
		typeMenuOpen = false;
		try { await addModulator(trackId, kind); }
		catch (e) { notify(`Erreur ajout modulateur : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	async function handleRemoveMod(modId: string) {
		try { await removeModulator(trackId, modId); }
		catch (e) { notify(`Erreur suppression : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	async function handleRemoveRoute(routeId: string) {
		try { await removeModulationRoute(trackId, routeId); }
		catch (e) { notify(`Erreur suppression route : ${(e as Error)?.message ?? e}`, 'error'); }
	}

	function assignByKey(modId: string, key: string) {
		const option = options.find((o) => o.key === key && o.listed);
		if (!option) return;
		const alreadyRouted = mod_routes.some(
			(r) => r.modulator_id === modId && findTargetOption(options, r.target)?.key === key,
		);
		if (alreadyRouted) {
			notify('Ce modulateur pilote déjà ce réglage', 'info');
			return;
		}
		handleAssign(modId, option);
	}

	async function handleDepthChange(route: ProjectModulationRoute, e: Event) {
		const slider = parseFloat((e.currentTarget as HTMLInputElement).value);
		try { await setModulationDepth(trackId, route.id, slider * depthScale(route, options)); }
		catch (err) { notify(`Erreur profondeur : ${(err as Error)?.message ?? err}`, 'error'); }
	}
</script>

<div class="mod-panel" class:compact>

	<!-- ── Sources ─────────────────────────────────────────────────────────────── -->
	<div class="section-header">
		<span class="section-title">{compact ? 'Mod' : 'Modulateurs'}</span>
		<button
			class="add-btn"
			class:active={typeMenuOpen}
			type="button"
			title="Ajouter un modulateur"
			aria-haspopup="menu"
			aria-expanded={typeMenuOpen}
			on:mousedown|stopPropagation
			on:click={() => (typeMenuOpen = !typeMenuOpen)}
		>+</button>
		{#if typeMenuOpen}
			<ModulatorTypeMenu on:select={(e) => handleAddModulator(e.detail)} on:close={() => (typeMenuOpen = false)} />
		{/if}
	</div>

	{#if modulators.length === 0}
		<p class="empty">{compact ? '—' : 'Aucun modulateur — cliquez + pour en ajouter'}</p>
	{/if}

	{#each modulators as mod, i (mod.id)}
		{@const card = {
			trackId,
			mod,
			label: modulatorLabel(modulators, mod),
			color: modulatorColor(i),
			pickerOpen: pickerModulatorId === mod.id,
		}}
		<div class="slot">
			{#if mod.kind === 'envelope'}
				<EnvelopeCard {...card} hasInstrument={instrument !== null}
					on:assign={(e) => assignByKey(mod.id, e.detail)}
					on:togglepicker={() => togglePicker(mod.id)}
					on:remove={() => handleRemoveMod(mod.id)} />
			{:else if mod.kind === 'audio_follower'}
				<FollowerCard {...card} {effects}
					on:assign={(e) => assignByKey(mod.id, e.detail)}
					on:togglepicker={() => togglePicker(mod.id)}
					on:remove={() => handleRemoveMod(mod.id)} />
			{:else if mod.kind === 'macro'}
				<MacroCard {...card} routeCount={mod_routes.filter((r) => r.modulator_id === mod.id).length}
					on:assign={(e) => assignByKey(mod.id, e.detail)}
					on:togglepicker={() => togglePicker(mod.id)}
					on:remove={() => handleRemoveMod(mod.id)} />
			{:else}
				<LfoCard {...card}
					on:assign={(e) => assignByKey(mod.id, e.detail)}
					on:togglepicker={() => togglePicker(mod.id)}
					on:remove={() => handleRemoveMod(mod.id)} />
			{/if}
			{#if pickerModulatorId === mod.id}
				<ModulationTargetPicker
					options={pickerOptions}
					loading={loadingParams}
					on:select={(e) => handleAssign(mod.id, e.detail)}
					on:close={() => (pickerModulatorId = null)}
				/>
			{/if}
		</div>
	{/each}

	<!-- ── Routes ──────────────────────────────────────────────────────────────── -->
	<div class="section-header routes-header">
		<span class="section-title">{compact ? 'Rtes' : 'Routes'}</span>
	</div>

	{#if mod_routes.length === 0}
		<p class="empty">{compact ? '—' : "Aucune route — glissez l'icône d'un modulateur sur un réglage"}</p>
	{/if}

	{#each mod_routes as route (route.id)}
		{@const routeColor = colorForRoute(route)}
		<div class="slot">
			<!-- svelte-ignore a11y-no-static-element-interactions -->
			<div class="route-row" style:border-left-color={routeColor}>
				<span class="route-src" style:color={routeColor}>{modNameForRoute(route)}</span>
				<span class="route-arrow">→</span>
				<span class="route-tgt" title={routeTargetLabel(route, options)}>{routeTargetLabel(route, options)}</span>
				<input
					class="depth-slider"
					type="range"
					min="-1"
					max="1"
					step="0.01"
					value={sliderDepth(route, options)}
					on:change={(e) => handleDepthChange(route, e)}
					style:accent-color={routeColor}
				/>
				<span class="depth-val">{formatDepth(sliderDepth(route, options))}</span>
				<button class="rm-btn" type="button"
					on:click|stopPropagation={() => handleRemoveRoute(route.id)} title="Supprimer">×</button>
			</div>
		</div>
	{/each}

</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.mod-panel {
		width: 100%;

		&.compact .section-header { padding: 3px 5px; }
		&.compact .route-row { padding: 2px 5px; gap: 3px; }
		&.compact .empty { padding: 6px 5px; font-size: 9px; }
		&.compact .depth-slider { width: 40px; }
		&.compact .depth-val { min-width: 24px; font-size: 8px; }
		&.compact .route-src { min-width: 22px; font-size: 8px; }
		&.compact .route-tgt { font-size: 8px; }
	}

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
		&:hover, &.active { background: rgba(21,113,255,0.15); color: $text-color; border-color: rgba(21,113,255,0.4); }
	}

	.empty {
		font-size: 10px; color: $text-faint; text-align: center;
		padding: 12px 8px; margin: 0;
	}

	.slot { border-bottom: 1px solid $border-faint; }

	.rm-btn {
		width: 16px; height: 16px; border: none; background: transparent;
		color: $text-faint; font-size: 14px; line-height: 1; cursor: pointer;
		padding: 0; display: flex; align-items: center; justify-content: center;
		flex-shrink: 0;
		&:hover { color: #ff4136; }
	}

	// Routes
	.route-row {
		display: flex; align-items: center; gap: 5px;
		padding: 5px 6px 5px 4px;
		border-left: 2px solid transparent;
		&:hover { background: $surface-1; }
	}

	.route-src {
		flex-shrink: 0;
		min-width: 36px;
		font-size: 9px; font-family: $font-mono; font-weight: 700;
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
