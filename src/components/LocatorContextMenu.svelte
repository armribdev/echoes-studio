<script lang="ts">
	import { createEventDispatcher, onMount } from 'svelte';
	import { dismissable } from '../frontend/dismiss';

	export let x = 0;
	export let y = 0;
	/** Empty lane click → add actions. Locator → edit actions. */
	export let mode: 'empty' | 'locator' = 'empty';
	export let kind: 'marker' | 'region' | null = null;
	export let name = '';

	const dispatch = createEventDispatcher<{
		addMarker: void;
		addRegion: void;
		goTo: void;
		rename: void;
		setLoop: void;
		delete: void;
		close: void;
	}>();

	let menuEl: HTMLDivElement;
	let adjustedX = x;
	let adjustedY = y;

	onMount(() => {
		if (!menuEl) return;
		const rect = menuEl.getBoundingClientRect();
		const vw = window.innerWidth;
		const vh = window.innerHeight;
		if (x + rect.width > vw) adjustedX = vw - rect.width - 8;
		if (y + rect.height > vh) adjustedY = vh - rect.height - 8;
	});

	function close() {
		dispatch('close');
	}
</script>

<div
	class="context-menu"
	bind:this={menuEl}
	use:dismissable={close}
	style="left: {adjustedX}px; top: {adjustedY}px"
>
	{#if mode === 'empty'}
		<button
			class="item"
			on:click={() => {
				dispatch('addMarker');
				close();
			}}
		>
			<span>Ajouter un marqueur</span>
			<span class="item-shortcut">M</span>
		</button>
		<button
			class="item"
			on:click={() => {
				dispatch('addRegion');
				close();
			}}
		>
			<span>Ajouter une région</span>
			<span class="item-shortcut">R</span>
		</button>
	{:else}
		<div class="menu-header">{name || (kind === 'region' ? 'Région' : 'Marqueur')}</div>
		<div class="separator"></div>
		<button
			class="item"
			on:click={() => {
				dispatch('goTo');
				close();
			}}
		>
			<span>Aller ici</span>
		</button>
		<button
			class="item"
			on:click={() => {
				dispatch('rename');
				close();
			}}
		>
			<span>Renommer</span>
		</button>
		{#if kind === 'region'}
			<button
				class="item"
				on:click={() => {
					dispatch('setLoop');
					close();
				}}
			>
				<span>Boucler cette région</span>
			</button>
		{/if}
		<div class="separator"></div>
		<button
			class="item item--danger"
			on:click={() => {
				dispatch('delete');
				close();
			}}
		>
			<span>Supprimer</span>
		</button>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.context-menu {
		position: fixed;
		z-index: 1000;
		min-width: 200px;
		padding: 4px;
		background: $panel-background-color;
		border: 1px solid $border-color;
		border-radius: $radius-sm;
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
		user-select: none;
	}

	.menu-header {
		padding: 6px 10px 4px;
		font-size: 11px;
		color: $text-subtle;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.separator {
		height: 1px;
		margin: 4px 6px;
		background: $border-color;
	}

	.item {
		width: 100%;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
		padding: 6px 10px;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: $text-color;
		font-size: 12px;
		font-family: inherit;
		cursor: pointer;
		text-align: left;

		&:hover {
			background: rgba(21, 113, 255, 0.15);
		}

		&--danger:hover {
			background: rgba(239, 68, 68, 0.18);
			color: #fca5a5;
		}
	}

	.item-shortcut {
		font-size: 10px;
		color: $text-subtle;
		font-family: 'SF Mono', 'Fira Code', ui-monospace, monospace;
	}
</style>
