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
		TrackEffect
	} from '../types/global';
	import {
		addModulator,
		removeModulator,
		addModulationRoute,
		removeModulationRoute,
		setModulationDepth
	} from '../frontend/commands/dispatcher';
	import {
		buildTargetOptions,
		fetchPluginParams,
		findTargetOption,
		pluginPlacements,
		type ModulationTargetOption,
		type PluginParamInfo,
		type PluginPlacement
	} from '../frontend/modulation/targets';
	import { instrumentDisplayName, plugins } from '../stores/plugins';
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

	$: instrumentName = instrumentDisplayName($plugins, instrument);
	$: placements = pluginPlacements(trackId, effects, instrumentName);
	$: placementSignature = placements.map((p) => `${p.key}=${p.name}`).join('|');
	$: (placementSignature, loadPluginParams(placements));
	$: options = buildTargetOptions(effects, placements, paramsByPlacement);
	$: pickerOptions = options.filter((o) => o.listed);

	async function loadPluginParams(list: PluginPlacement[]) {
		const request = ++paramsRequest;
		loadingParams = true;
		const entries = await Promise.all(
			list.map(async (p) => [p.key, await fetchPluginParams(p.key)] as const)
		);
		if (request !== paramsRequest) return;
		paramsByPlacement = Object.fromEntries(entries);
		loadingParams = false;
	}

	function routeTargetLabel(route: ProjectModulationRoute, list: ModulationTargetOption[]): string {
		const option = findTargetOption(list, route.target);
		if (option)
			return option.kind === 'track_gain' ? option.label : `${option.group} › ${option.label}`;
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
			await addModulationRoute(
				trackId,
				modId,
				option.kind,
				option.param,
				DEFAULT_DEPTH * option.depthScale,
				option.effectId
			);
		} catch (e) {
			notify(`Erreur assignation : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function handleAddModulator(kind: ModulatorKind) {
		typeMenuOpen = false;
		try {
			await addModulator(trackId, kind);
		} catch (e) {
			notify(`Erreur ajout modulateur : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function handleRemoveMod(modId: string) {
		try {
			await removeModulator(trackId, modId);
		} catch (e) {
			notify(`Erreur suppression : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	async function handleRemoveRoute(routeId: string) {
		try {
			await removeModulationRoute(trackId, routeId);
		} catch (e) {
			notify(`Erreur suppression route : ${(e as Error)?.message ?? e}`, 'error');
		}
	}

	function assignByKey(modId: string, key: string) {
		const option = options.find((o) => o.key === key && o.listed);
		if (!option) return;
		const alreadyRouted = mod_routes.some(
			(r) => r.modulator_id === modId && findTargetOption(options, r.target)?.key === key
		);
		if (alreadyRouted) {
			notify('Ce modulateur pilote déjà ce réglage', 'info');
			return;
		}
		handleAssign(modId, option);
	}

	async function handleDepthChange(route: ProjectModulationRoute, e: Event) {
		const slider = parseFloat((e.currentTarget as HTMLInputElement).value);
		try {
			await setModulationDepth(trackId, route.id, slider * depthScale(route, options));
		} catch (err) {
			notify(`Erreur profondeur : ${(err as Error)?.message ?? err}`, 'error');
		}
	}

	// Bipolar track: the fill runs from the middle to the thumb.
	function paintBipolar(input: HTMLInputElement, value: number) {
		const position = ((value + 1) / 2) * 100;
		input.style.setProperty('--fill-start', `${Math.min(50, position)}%`);
		input.style.setProperty('--fill', `${Math.max(50, position)}%`);
	}

	function bipolarFill(input: HTMLInputElement, value: number) {
		paintBipolar(input, value);
		return { update: (next: number) => paintBipolar(input, next) };
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
			on:click={() => (typeMenuOpen = !typeMenuOpen)}>+</button
		>
		{#if typeMenuOpen}
			<ModulatorTypeMenu
				on:select={(e) => handleAddModulator(e.detail)}
				on:close={() => (typeMenuOpen = false)}
			/>
		{/if}
	</div>

	{#if modulators.length === 0}
		<p class="empty">{compact ? '—' : 'Aucun modulateur — cliquez + pour en ajouter'}</p>
	{/if}

	{#each modulators as mod, i (mod.id)}
		{@const modRoutes = mod_routes.filter((r) => r.modulator_id === mod.id)}
		{@const card = {
			trackId,
			mod,
			label: modulatorLabel(modulators, mod),
			color: modulatorColor(i),
			pickerOpen: pickerModulatorId === mod.id
		}}
		<div class="slot">
			{#if mod.kind === 'envelope'}
				<EnvelopeCard
					{...card}
					hasInstrument={instrument !== null}
					on:assign={(e) => assignByKey(mod.id, e.detail)}
					on:togglepicker={() => togglePicker(mod.id)}
					on:remove={() => handleRemoveMod(mod.id)}
				/>
			{:else if mod.kind === 'audio_follower'}
				<FollowerCard
					{...card}
					{effects}
					on:assign={(e) => assignByKey(mod.id, e.detail)}
					on:togglepicker={() => togglePicker(mod.id)}
					on:remove={() => handleRemoveMod(mod.id)}
				/>
			{:else if mod.kind === 'macro'}
				<MacroCard
					{...card}
					routeCount={modRoutes.length}
					on:assign={(e) => assignByKey(mod.id, e.detail)}
					on:togglepicker={() => togglePicker(mod.id)}
					on:remove={() => handleRemoveMod(mod.id)}
				/>
			{:else}
				<LfoCard
					{...card}
					on:assign={(e) => assignByKey(mod.id, e.detail)}
					on:togglepicker={() => togglePicker(mod.id)}
					on:remove={() => handleRemoveMod(mod.id)}
				/>
			{/if}
			{#if pickerModulatorId === mod.id}
				<ModulationTargetPicker
					options={pickerOptions}
					loading={loadingParams}
					on:select={(e) => handleAssign(mod.id, e.detail)}
					on:close={() => (pickerModulatorId = null)}
				/>
			{/if}
			{#each modRoutes as route (route.id)}
				<div class="route-row" style:border-left-color={card.color}>
					<span class="route-tgt" title={routeTargetLabel(route, options)}
						>{routeTargetLabel(route, options)}</span
					>
					<input
						class="depth-slider"
						type="range"
						min="-1"
						max="1"
						step="0.01"
						value={sliderDepth(route, options)}
						on:input={(e) => paintBipolar(e.currentTarget, e.currentTarget.valueAsNumber)}
						on:change={(e) => handleDepthChange(route, e)}
						style:--range-color={card.color}
						use:bipolarFill={sliderDepth(route, options)}
					/>
					<span class="depth-val">{formatDepth(sliderDepth(route, options))}</span>
					<button
						class="rm-btn"
						type="button"
						on:click|stopPropagation={() => handleRemoveRoute(route.id)}
						title="Supprimer">×</button
					>
				</div>
			{/each}
		</div>
	{/each}

	{#if modulators.length > 0 && mod_routes.length === 0}
		<p class="empty">
			{compact ? '—' : "Aucune route — glissez l'icône d'un modulateur sur un réglage"}
		</p>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.mod-panel {
		width: 100%;

		&.compact .depth-slider {
			width: 40px;
		}
		&.compact .depth-val {
			min-width: 24px;
		}
	}

	.section-header {
		display: flex;
		align-items: center;
		gap: $space-2;
		padding: $space-2 $panel-inset;
		border-bottom: 1px solid $border-faint;
		background: $panel-background-color;
		position: sticky;
		top: 0;
		z-index: 1;
	}

	.section-title {
		flex: 1;
		font-size: 11px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.08em;
		color: $text-secondary;
		text-transform: uppercase;
	}

	.add-btn {
		@include button-square;

		&.active {
			@include control-on;
		}
	}

	.empty {
		font-size: 10px;
		color: $text-faint;
		text-align: center;
		padding: $space-3 $panel-inset;
		margin: 0;
	}

	.slot {
		border-bottom: 1px solid $border-faint;
	}

	.rm-btn {
		@include icon-button($ctrl-height-xs);
		@include icon-button-danger;
		font-size: 14px;
	}

	// Routes
	.route-row {
		display: flex;
		align-items: center;
		gap: $space-2;
		padding: $space-1 $panel-inset $space-1 ($panel-inset + $space-2 - 2px);
		border-left: 2px solid transparent;
		&:hover {
			background: $surface-1;
		}
	}

	.route-tgt {
		flex: 1;
		font-size: 10px;
		font-family: $font-mono;
		color: $text-subtle;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		min-width: 0;
	}

	.depth-slider {
		@include range-slider;
		flex-shrink: 0;
		width: 52px;
	}

	.depth-val {
		flex-shrink: 0;
		min-width: 32px;
		text-align: right;
		font-size: 9px;
		font-family: $font-mono;
		color: $text-subtle;
	}
</style>
