<!-- EffectContextMenu.svelte — right-click menu of a plugin effect or track instrument -->
<script lang="ts">
	import { createEventDispatcher, onMount } from 'svelte';
	import { dismissable } from '../frontend/dismiss';
	import { preferences } from '../stores/preferences';

	export let x = 0;
	export let y = 0;
	export let pluginId: string;
	export let pluginName: string;

	const dispatch = createEventDispatcher<{ close: void }>();

	let menuEl: HTMLDivElement;
	let adjustedX = x;
	let adjustedY = y;

	onMount(() => {
		const rect = menuEl.getBoundingClientRect();
		if (x + rect.width > window.innerWidth) adjustedX = window.innerWidth - rect.width - 8;
		if (y + rect.height > window.innerHeight) adjustedY = window.innerHeight - rect.height - 8;
	});

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

<div
	class="context-menu"
	bind:this={menuEl}
	use:dismissable={() => dispatch('close')}
	style="left: {adjustedX}px; top: {adjustedY}px"
>
	<div class="menu-header">{pluginName}</div>
	<div class="separator"></div>

	<button class="item" type="button" disabled={mode !== 'chosen'} on:click={toggleAlwaysIsolated}>
		<span class="check">{alwaysIsolated ? '✓' : ''}</span>
		<span>Toujours isoler {pluginName}</span>
	</button>
	<p class="hint">
		{#if mode === 'off'}Isolation désactivée dans les Préférences.
		{:else if mode === 'all'}Tous les plugins sont déjà isolés (Préférences).
		{:else}Sur toutes les pistes et dans tous les projets : s'il plante, Echoes continue.{/if}
	</p>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.context-menu {
		position: fixed;
		z-index: 201;
		width: 240px;
		background: #26262e;
		border: 1px solid $border-color;
		border-radius: $radius-lg;
		padding: 4px;
		box-shadow: 0 8px 32px rgba(0, 0, 0, 0.55), 0 2px 8px rgba(0, 0, 0, 0.3);
		user-select: none;
	}

	.menu-header {
		padding: 4px 10px 6px;
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.6px;
		text-transform: uppercase;
		color: $text-faint;
		font-family: $font-mono;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.separator {
		height: 1px;
		margin: 4px 6px;
		background: $border-faint;
	}

	.item {
		width: 100%;
		height: 30px;
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 0 8px;
		border: none;
		background: none;
		color: rgba(247, 247, 249, 0.82);
		font-size: 12px;
		font-family: inherit;
		text-align: left;
		border-radius: $radius-sm;
		cursor: pointer;

		&:hover:not(:disabled) { background: rgba(21, 113, 255, 0.18); color: $text-color; }
		&:disabled { cursor: default; opacity: 0.45; }
	}

	.check {
		width: 14px;
		color: #4ade80;
		text-align: center;
	}

	.hint {
		margin: 2px 8px 6px 30px;
		font-size: 10px;
		line-height: 1.35;
		color: $text-faint;
	}
</style>
