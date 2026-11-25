<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';
	import { shortcutLabels } from '../frontend/shortcuts/label';
	import Menu from './Menu.svelte';
	import MenuHeader from './MenuHeader.svelte';
	import MenuItem from './MenuItem.svelte';
	import MenuSeparator from './MenuSeparator.svelte';

	export let x = 0;
	export let y = 0;
	export let selectionCount = 0;
	export let canPaste = false;
	export let canUnlink = false;

	const dispatch = createEventDispatcher<{
		cut: void;
		copy: void;
		paste: void;
		duplicate: void;
		unlink: void;
		delete: void;
		close: void;
	}>();

	type Action = 'cut' | 'copy' | 'paste' | 'duplicate' | 'unlink' | 'delete';

	function choose(action: Action) {
		dispatch(action);
		dispatch('close');
	}

	$: label =
		selectionCount === 1
			? $t('clipMenu.oneClip')
			: $t('clipMenu.nClips', { values: { n: selectionCount } });
</script>

<Menu {x} {y} on:close>
	<MenuHeader>{label}</MenuHeader>
	<MenuSeparator />

	<MenuItem shortcut={$shortcutLabels('arrangement.cut')} on:click={() => choose('cut')}>
		<svg
			slot="icon"
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
		{$t('clipMenu.cut')}
	</MenuItem>

	<MenuItem shortcut={$shortcutLabels('arrangement.copy')} on:click={() => choose('copy')}>
		<svg
			slot="icon"
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
		{$t('clipMenu.copy')}
	</MenuItem>

	<MenuItem
		disabled={!canPaste}
		shortcut={$shortcutLabels('arrangement.paste')}
		on:click={() => choose('paste')}
	>
		<svg
			slot="icon"
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
		{$t('clipMenu.paste')}
	</MenuItem>

	<MenuSeparator />

	<MenuItem
		shortcut={$shortcutLabels('arrangement.duplicate')}
		on:click={() => choose('duplicate')}
	>
		<svg
			slot="icon"
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
		{$t('clipMenu.duplicate')}
	</MenuItem>

	{#if canUnlink}
		<MenuItem on:click={() => choose('unlink')}>{$t('clipMenu.makeIndependent')}</MenuItem>
	{/if}

	<MenuSeparator />

	<MenuItem
		danger
		shortcut={$shortcutLabels('arrangement.delete')}
		on:click={() => choose('delete')}
	>
		<svg
			slot="icon"
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
		{$t('common.delete')}
	</MenuItem>
</Menu>
