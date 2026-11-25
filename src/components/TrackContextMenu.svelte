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

	type Action = 'addtrack' | 'rename' | 'automate' | 'insertsound';

	function choose(action: Action) {
		dispatch(action);
		dispatch('close');
	}

	function deleteTrack() {
		dispatch('delete', { trackId });
		dispatch('close');
	}
</script>

<Menu {x} {y} on:close>
	<MenuHeader>{trackName}</MenuHeader>
	<MenuSeparator />

	<MenuItem shortcut={$shortcutLabels('track.add')} on:click={() => choose('addtrack')}>
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
			<line x1="12" y1="5" x2="12" y2="19" />
			<line x1="5" y1="12" x2="19" y2="12" />
		</svg>
		{$t('trackMenu.newTrack')}
	</MenuItem>

	<MenuItem on:click={() => choose('insertsound')}>
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
			<circle cx="11" cy="11" r="7" />
			<line x1="21" y1="21" x2="16.65" y2="16.65" />
			<path d="M8 11h6M11 8v6" />
		</svg>
		{$t('trackMenu.insertSound')}
	</MenuItem>

	<MenuSeparator />

	<MenuItem shortcut="F2" on:click={() => choose('rename')}>
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
			<path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
			<path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" />
		</svg>
		{$t('common.rename')}
	</MenuItem>

	<MenuItem on:click={() => choose('automate')}>
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
			<polyline points="3 17 9 11 13 15 21 7" />
			<circle cx="9" cy="11" r="1.5" />
			<circle cx="13" cy="15" r="1.5" />
		</svg>
		{$t('trackMenu.automateParam')}
	</MenuItem>

	<MenuSeparator />

	<MenuItem danger shortcut={$shortcutLabels('arrangement.delete')} on:click={deleteTrack}>
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
		{$t('trackMenu.deleteTrack')}
	</MenuItem>
</Menu>
