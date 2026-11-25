<!-- EffectContextMenu.svelte — right-click menu of a plugin effect or track instrument -->
<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { preferences } from '../stores/preferences';
	import Menu from './Menu.svelte';
	import MenuHeader from './MenuHeader.svelte';
	import MenuItem from './MenuItem.svelte';
	import MenuSeparator from './MenuSeparator.svelte';

	export let x = 0;
	export let y = 0;
	export let pluginId: string;
	export let pluginName: string;

	const dispatch = createEventDispatcher<{ close: void }>();

	$: mode = $preferences.audio.pluginIsolation;
	$: alwaysIsolated = $preferences.audio.alwaysIsolated.includes(pluginId);

	function toggleAlwaysIsolated() {
		preferences.update((prefs) => {
			const others = prefs.audio.alwaysIsolated.filter((id) => id !== pluginId);
			const list = alwaysIsolated ? others : [...others, pluginId];
			return { ...prefs, audio: { ...prefs.audio, alwaysIsolated: list } };
		});
		dispatch('close');
	}
</script>

<Menu {x} {y} style="width: 240px" on:close>
	<MenuHeader>{pluginName}</MenuHeader>
	<MenuSeparator />

	<MenuItem checked={alwaysIsolated} disabled={mode !== 'chosen'} on:click={toggleAlwaysIsolated}>
		Toujours isoler {pluginName}
	</MenuItem>
	<p class="hint">
		{#if mode === 'off'}Isolation désactivée dans les Préférences.
		{:else if mode === 'all'}Tous les plugins sont déjà isolés (Préférences).
		{:else}Sur toutes les pistes et dans tous les projets : s'il plante, Echoes continue.{/if}
	</p>
</Menu>

<style lang="scss">
	@use '../variables.scss' as *;

	.hint {
		margin: 2px 8px 6px 28px;
		font-size: 10px;
		line-height: 1.35;
		color: $text-faint;
	}
</style>
