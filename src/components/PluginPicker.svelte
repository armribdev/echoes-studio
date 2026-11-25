<!-- PluginPicker.svelte — modal central pour parcourir et insérer un plugin -->
<script lang="ts">
	import { pluginFormatColor } from '../frontend/pluginFormatColor';
	import { createEventDispatcher, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import {
		BUILTIN_EFFECT_PLUGINS,
		BUILTIN_INSTRUMENT_PLUGIN,
		BUILTIN_SAMPLER_PLUGIN,
		blockedPlugins,
		plugins,
		pluginsLoading,
		reenableBlockedPlugin,
		refreshBlockedPlugins,
		rescanPlugins
	} from '../stores/plugins';
	import type { PluginInfo } from '../stores/plugins';
	import Dialog from './Dialog.svelte';

	const dispatch = createEventDispatcher<{ pick: PluginInfo; cancel: void }>();

	/** `instrument` = synths only, `effect` = inserts only, `all` = no filter */
	export let mode: 'instrument' | 'effect' | 'all' = 'all';

	let query = '';
	let selectedIndex = 0;
	let itemEls: HTMLButtonElement[] = [];
	let showBlocked = false;

	$: if ($blockedPlugins.length === 0) showBlocked = false;

	export let title: string | null = null;

	$: builtins =
		mode === 'effect'
			? BUILTIN_EFFECT_PLUGINS
			: mode === 'instrument'
				? [BUILTIN_INSTRUMENT_PLUGIN, BUILTIN_SAMPLER_PLUGIN]
				: [];

	$: scanned = $plugins
		.filter((p) => {
			if (mode === 'instrument' && !(p.is_instrument ?? false)) return false;
			// The engine hosts CLAP and VST3 instruments (ADR-013).
			if (mode === 'instrument' && p.format !== 'CLAP' && p.format !== 'VST3') return false;
			if (mode === 'effect' && (p.is_instrument ?? false)) return false;
			return true;
		})
		.sort((a, b) => a.name.localeCompare(b.name));

	$: filtered = [...builtins, ...scanned].filter((p) => matchesQuery(p, query));

	$: pickerTitle =
		title ?? (mode === 'instrument' ? 'Instruments' : mode === 'effect' ? 'Effets' : 'Plugins');

	function matchesQuery(p: PluginInfo, text: string): boolean {
		if (!text) return true;
		const q = text.toLowerCase();
		return (
			p.name.toLowerCase().includes(q) ||
			p.vendor.toLowerCase().includes(q) ||
			p.format.toLowerCase().includes(q)
		);
	}

	$: if (filtered) {
		selectedIndex = 0;
	}

	onMount(() => {
		void refreshBlockedPlugins();
		window.addEventListener('keydown', handleKey);
		return () => window.removeEventListener('keydown', handleKey);
	});

	function pick(p: PluginInfo) {
		dispatch('pick', p);
	}

	function handleKey(e: KeyboardEvent) {
		if (showBlocked) return;
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			selectedIndex = Math.min(selectedIndex + 1, filtered.length - 1);
			itemEls[selectedIndex]?.scrollIntoView({ block: 'nearest' });
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			selectedIndex = Math.max(selectedIndex - 1, 0);
			itemEls[selectedIndex]?.scrollIntoView({ block: 'nearest' });
		} else if (e.key === 'Enter' && filtered[selectedIndex]) {
			pick(filtered[selectedIndex]);
		}
	}
</script>

<Dialog
	label="Plugin browser"
	width={420}
	height={540}
	initialFocus="first"
	on:close={() => dispatch('cancel')}
>
	<div class="header">
		<span class="title">{pickerTitle}</span>
		{#if $blockedPlugins.length > 0}
			<button class="rescan" type="button" on:click={() => (showBlocked = !showBlocked)}>
				{showBlocked
					? $t('blockedPlugins.back')
					: $t('blockedPlugins.show', { values: { n: $blockedPlugins.length } })}
			</button>
		{/if}
		<button class="rescan" type="button" on:click={rescanPlugins} disabled={$pluginsLoading}>
			{$pluginsLoading ? '…' : '↺'} Rescanner
		</button>
	</div>

	{#if showBlocked}
		<ul class="list blocked">
			{#each $blockedPlugins as blocked (blocked.path)}
				<li class="item" title={blocked.path}>
					<span class="plug-name">{blocked.name}</span>
					<span class="plug-vendor">
						{blocked.reason === 'crashed'
							? $t('blockedPlugins.crashed')
							: $t('blockedPlugins.timedOut')}
					</span>
					<button
						class="rescan"
						type="button"
						on:click={() => reenableBlockedPlugin(blocked.path)}
						disabled={$pluginsLoading}
					>
						{$t('blockedPlugins.reenable')}
					</button>
				</li>
			{/each}
		</ul>
	{:else}
		<input class="search" type="text" placeholder="Rechercher…" bind:value={query} />

		<div class="list" role="listbox">
			{#if filtered.length === 0}
				<p class="empty">
					{$pluginsLoading
						? 'Scan en cours…'
						: query
							? 'Aucun résultat'
							: mode === 'instrument'
								? 'Aucun instrument détecté'
								: 'Aucun plugin détecté'}
				</p>
			{:else}
				{#each filtered as p, i (p.id)}
					<button
						class="item"
						class:selected={i === selectedIndex}
						type="button"
						role="option"
						aria-selected={i === selectedIndex}
						bind:this={itemEls[i]}
						on:click={() => pick(p)}
						on:pointerenter={() => (selectedIndex = i)}
					>
						<span class="plug-name">{p.name}</span>
						<span class="plug-vendor">{p.vendor || '—'}</span>
						<span class="badge" style:background={pluginFormatColor(p.format)}>{p.format}</span>
					</button>
				{/each}
			{/if}
		</div>
	{/if}
</Dialog>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.header {
		display: flex;
		align-items: center;
		padding: 10px 12px 8px;
		border-bottom: 1px solid $border-faint;
		gap: 8px;
	}

	.title {
		flex: 1;
		font-size: 11px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: $text-muted;
	}

	.rescan {
		@include chip($ctrl-height-sm);
	}

	.search {
		@include field($ctrl-height-sm, 12px);
		margin: 8px 10px;
	}

	.list {
		flex: 1;
		overflow-y: auto;
		padding: 0 6px 6px;
	}

	.blocked {
		margin: 0;
		list-style: none;

		.rescan {
			flex-shrink: 0;
		}
	}

	.empty {
		font-size: 11px;
		color: $text-faint;
		text-align: center;
		padding: 24px;
		margin: 0;
	}

	.item {
		@include list-item($menu-item-height, 11px);
		gap: 6px;

		&.selected {
			@include list-item-selected;
		}
	}

	.plug-name {
		flex: 1;
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.plug-vendor {
		font-size: 10px;
		color: $text-faint;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 100px;
	}

	.badge {
		font-size: 8px;
		font-family: $font-mono;
		font-weight: 700;
		letter-spacing: 0.04em;
		padding: 2px 5px;
		border-radius: $radius-xs;
		color: $on-solid;
		flex-shrink: 0;
	}
</style>
