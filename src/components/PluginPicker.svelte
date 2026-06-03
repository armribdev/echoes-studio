<!-- PluginPicker.svelte — modal to browse and insert a VST3/CLAP plugin -->
<script lang="ts">
	import { createEventDispatcher, onMount } from 'svelte';
	import { plugins, pluginsLoading, rescanPlugins } from '../stores/plugins';
	import type { PluginInfo } from '../stores/plugins';

	const dispatch = createEventDispatcher<{ pick: PluginInfo; cancel: void }>();

	/** `instrument` = synths only, `effect` = inserts only, `all` = no filter */
	export let mode: 'instrument' | 'effect' | 'all' = 'all';

	let query = '';
	let inputEl: HTMLInputElement;

	$: filtered = $plugins
		.filter((p) => {
			if (mode === 'instrument' && !(p.is_instrument ?? false)) return false;
			if (mode === 'effect' && (p.is_instrument ?? false)) return false;
			if (!query) return true;
			const q = query.toLowerCase();
			return (
				p.name.toLowerCase().includes(q) ||
				p.vendor.toLowerCase().includes(q) ||
				p.format.toLowerCase().includes(q)
			);
		})
		.sort((a, b) => a.name.localeCompare(b.name));

	$: pickerTitle = mode === 'instrument' ? 'Instruments' : mode === 'effect' ? 'Effets' : 'Plugins';

	onMount(() => inputEl?.focus());

	function pick(p: PluginInfo) {
		dispatch('pick', p);
	}

	function handleKey(e: KeyboardEvent) {
		if (e.key === 'Escape') dispatch('cancel');
	}

	const FORMAT_COLOR: Record<string, string> = {
		VST3: '#1571ff',
		CLAP: '#22c55e',
		AU:   '#f59e0b',
	};
</script>

<!-- Backdrop -->
<div class="backdrop" on:pointerdown|self={() => dispatch('cancel')} role="presentation">
	<div class="picker" on:keydown={handleKey} role="dialog" aria-label="Plugin browser">
		<div class="header">
			<span class="title">{pickerTitle}</span>
			<button class="rescan" type="button" on:click={rescanPlugins} disabled={$pluginsLoading}>
				{$pluginsLoading ? '…' : '↺'} Rescanner
			</button>
		</div>

		<input
			bind:this={inputEl}
			class="search"
			type="text"
			placeholder="Rechercher…"
			bind:value={query}
		/>

		<div class="list" role="listbox">
			{#if filtered.length === 0}
				<p class="empty">
					{$pluginsLoading
						? 'Scan en cours…'
						: query
							? 'Aucun résultat'
							: mode === 'instrument'
								? 'Aucun instrument détecté (CLAP)'
								: 'Aucun plugin détecté'}
				</p>
			{:else}
				{#each filtered as p (p.id)}
					<button
						class="item"
						type="button"
						role="option"
						aria-selected="false"
						on:click={() => pick(p)}
					>
						<span class="plug-name">{p.name}</span>
						<span class="plug-vendor">{p.vendor || '—'}</span>
						<span
							class="badge"
							style:background={FORMAT_COLOR[p.format] ?? '#555'}
						>{p.format}</span>
					</button>
				{/each}
			{/if}
		</div>
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.backdrop {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.6);
		z-index: 2000;
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.picker {
		width: 420px;
		max-height: 540px;
		background: #1e1e26;
		border: 1px solid $border-color;
		border-radius: $radius-lg;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		box-shadow: 0 12px 40px rgba(0, 0, 0, 0.6);
	}

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
		font-size: 10px;
		font-family: $font-mono;
		padding: 3px 7px;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		background: transparent;
		color: $text-subtle;
		cursor: pointer;
		&:hover:not(:disabled) { background: $surface-hover; color: $text-color; }
		&:disabled { opacity: 0.4; cursor: default; }
	}

	.search {
		margin: 8px 10px;
		padding: 6px 8px;
		background: #151519;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		color: $text-color;
		font-size: 12px;
		outline: none;
		&:focus { border-color: rgba(21, 113, 255, 0.5); }
	}

	.list {
		flex: 1;
		overflow-y: auto;
		padding: 0 6px 6px;
	}

	.empty {
		font-size: 11px;
		color: $text-faint;
		text-align: center;
		padding: 24px;
		margin: 0;
	}

	.item {
		display: flex;
		align-items: center;
		gap: 6px;
		width: 100%;
		padding: 6px 8px;
		border: none;
		border-radius: $radius-sm;
		background: transparent;
		color: $text-color;
		font-size: 11px;
		font-family: inherit;
		text-align: left;
		cursor: pointer;
		&:hover { background: $surface-hover; }
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
		border-radius: 3px;
		color: #fff;
		flex-shrink: 0;
	}
</style>
