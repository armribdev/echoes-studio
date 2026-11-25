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

	type Action = 'addMarker' | 'addRegion' | 'goTo' | 'rename' | 'setLoop' | 'delete';

	function choose(action: Action) {
		dispatch(action);
		dispatch('close');
	}
</script>

<Menu {x} {y} on:close>
	{#if mode === 'empty'}
		<MenuItem
			shortcut={$shortcutLabels('arrangement.addMarker')}
			on:click={() => choose('addMarker')}
		>
			{$t('locatorMenu.addMarker')}
		</MenuItem>
		<MenuItem
			shortcut={$shortcutLabels('arrangement.addRegion')}
			on:click={() => choose('addRegion')}
		>
			{$t('locatorMenu.addRegion')}
		</MenuItem>
	{:else}
		<MenuHeader>
			{name || (kind === 'region' ? $t('locatorMenu.region') : $t('locatorMenu.marker'))}
		</MenuHeader>
		<MenuSeparator />
		<MenuItem on:click={() => choose('goTo')}>{$t('locatorMenu.goHere')}</MenuItem>
		<MenuItem on:click={() => choose('rename')}>{$t('common.rename')}</MenuItem>
		{#if kind === 'region'}
			<MenuItem on:click={() => choose('setLoop')}>{$t('locatorMenu.loopRegion')}</MenuItem>
		{/if}
		<MenuSeparator />
		<MenuItem danger on:click={() => choose('delete')}>{$t('common.delete')}</MenuItem>
	{/if}
</Menu>
