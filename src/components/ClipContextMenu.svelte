<script lang="ts">
	import { createEventDispatcher, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { dismissable } from '../frontend/dismiss';

	export let x = 0;
	export let y = 0;
	export let selectionCount = 0;
	export let canPaste = false;

	const dispatch = createEventDispatcher<{
		cut: void;
		copy: void;
		paste: void;
		duplicate: void;
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

	$: label =
		selectionCount === 1
			? $t('clipMenu.oneClip')
			: $t('clipMenu.nClips', { values: { n: selectionCount } });
</script>

<div
	class="context-menu"
	bind:this={menuEl}
	use:dismissable={close}
	style="left: {adjustedX}px; top: {adjustedY}px"
>
	<div class="menu-header">{label}</div>

	<div class="separator"></div>

	<button
		class="item"
		on:click={() => {
			dispatch('cut');
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
			<circle cx="6" cy="6" r="3" />
			<circle cx="6" cy="18" r="3" />
			<line x1="20" y1="4" x2="8.73" y2="15.27" />
			<line x1="10.27" y1="11.27" x2="20" y2="20" />
		</svg>
		<span>{$t('clipMenu.cut')}</span>
		<span class="item-shortcut">Ctrl+X</span>
	</button>

	<button
		class="item"
		on:click={() => {
			dispatch('copy');
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
			<rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
			<path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
		</svg>
		<span>{$t('clipMenu.copy')}</span>
		<span class="item-shortcut">Ctrl+C</span>
	</button>

	<button
		class="item"
		class:item--disabled={!canPaste}
		on:click={() => {
			if (canPaste) {
				dispatch('paste');
				close();
			}
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
			<path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" />
			<rect x="8" y="2" width="8" height="4" rx="1" ry="1" />
		</svg>
		<span>{$t('clipMenu.paste')}</span>
		<span class="item-shortcut">Ctrl+V</span>
	</button>

	<div class="separator"></div>

	<button
		class="item"
		on:click={() => {
			dispatch('duplicate');
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
			<rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
			<path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
		</svg>
		<span>{$t('clipMenu.duplicate')}</span>
		<span class="item-shortcut">Ctrl+D</span>
	</button>

	<div class="separator"></div>

	<button
		class="item item--danger"
		on:click={() => {
			dispatch('delete');
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
		<span>{$t('common.delete')}</span>
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

		&--disabled {
			opacity: 0.35;
			cursor: default;

			&:hover {
				background: none;
				color: rgba(247, 247, 249, 0.82);

				.item-icon {
					color: $text-subtle;
				}
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
