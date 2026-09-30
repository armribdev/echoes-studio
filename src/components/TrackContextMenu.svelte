<script lang="ts">
	import { createEventDispatcher, onMount } from 'svelte';
	import { dismissable } from '../frontend/dismiss';

	export let x = 0;
	export let y = 0;
	export let trackId = '';
	export let trackName = '';

	const dispatch = createEventDispatcher<{
		addtrack: void;
		rename: void;
		automate: void;
		insertsound: void;
		delete: { trackId: string };
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
	<div class="menu-header">{trackName}</div>

	<div class="separator"></div>

	<button
		class="item"
		on:click={() => {
			dispatch('addtrack');
			close();
		}}
	>
		<svg
			class="item-icon"
			width="14"
			height="14"
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
			stroke-linecap="round"
			stroke-linejoin="round"
		>
			<line x1="12" y1="5" x2="12" y2="19" />
			<line x1="5" y1="12" x2="19" y2="12" />
		</svg>
		<span>Nouvelle piste</span>
		<span class="item-shortcut">Ctrl+Shift+T</span>
	</button>

	<button
		class="item"
		on:click={() => {
			dispatch('insertsound');
			close();
		}}
	>
		<svg
			class="item-icon"
			width="14"
			height="14"
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
			stroke-linecap="round"
			stroke-linejoin="round"
		>
			<circle cx="11" cy="11" r="7" />
			<line x1="21" y1="21" x2="16.65" y2="16.65" />
			<path d="M8 11h6M11 8v6" />
		</svg>
		<span>Insérer un son…</span>
	</button>

	<div class="separator"></div>

	<button
		class="item"
		on:click={() => {
			dispatch('rename');
			close();
		}}
	>
		<svg
			class="item-icon"
			width="14"
			height="14"
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
			stroke-linecap="round"
			stroke-linejoin="round"
		>
			<path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
			<path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" />
		</svg>
		<span>Renommer</span>
		<span class="item-shortcut">F2</span>
	</button>

	<button
		class="item"
		on:click={() => {
			dispatch('automate');
			close();
		}}
	>
		<svg
			class="item-icon"
			width="14"
			height="14"
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
			stroke-linecap="round"
			stroke-linejoin="round"
		>
			<polyline points="3 17 9 11 13 15 21 7" />
			<circle cx="9" cy="11" r="1.5" />
			<circle cx="13" cy="15" r="1.5" />
		</svg>
		<span>Automatiser un réglage…</span>
	</button>

	<div class="separator"></div>

	<button
		class="item item--danger"
		on:click={() => {
			dispatch('delete', { trackId });
			close();
		}}
	>
		<svg
			class="item-icon"
			width="14"
			height="14"
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
			stroke-linecap="round"
			stroke-linejoin="round"
		>
			<polyline points="3 6 5 6 21 6" />
			<path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
			<path d="M10 11v6M14 11v6" />
			<path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
		</svg>
		<span>Supprimer la piste</span>
		<span class="item-shortcut">Suppr</span>
	</button>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.context-menu {
		position: fixed;
		z-index: 201;
		min-width: 210px;
		background: #26262e;
		border: 1px solid $border-color;
		border-radius: $radius-lg;
		padding: 4px;
		box-shadow:
			0 8px 32px rgba(0, 0, 0, 0.55),
			0 2px 8px rgba(0, 0, 0, 0.3);
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
		border-radius: $radius-sm;
		cursor: pointer;
		text-align: left;
		transition:
			background 0.08s,
			color 0.08s;

		&:hover {
			background: $surface-hover;
			color: $text-color;

			.item-icon {
				color: $text-color;
			}
		}

		&--danger {
			color: rgba(255, 65, 54, 0.82);

			.item-icon {
				color: rgba(255, 65, 54, 0.6);
			}

			&:hover {
				background: rgba(255, 65, 54, 0.1);
				color: $color-error;

				.item-icon {
					color: $color-error;
				}
			}
		}
	}

	.item-icon {
		color: $text-subtle;
		flex-shrink: 0;
		transition: color 0.08s;
	}

	.item-shortcut {
		margin-left: auto;
		font-size: 10px;
		color: $text-faint;
		font-family: $font-mono;
		white-space: nowrap;
	}

	.separator {
		height: 1px;
		background: $border-color;
		margin: 3px 0;
	}
</style>
