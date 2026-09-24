<!-- MissingPluginsDialog.svelte — one summary of unloadable plugins when a project opens -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { listen } from '@tauri-apps/api/event';
	import { invoke } from '@tauri-apps/api/core';
	import type { Project } from '../types/global';
	import {
		pluginDisplayName,
		rescanPlugins,
		UNAVAILABLE_REASON_LABEL,
		type UnavailablePlugin,
		type UnavailableReason,
	} from '../stores/plugins';

	interface MissingPlugin {
		pluginId: string;
		name: string;
		reason: UnavailableReason;
		usages: string[];
	}

	const INSTRUMENT_KEY_PREFIX = 'instrument:';

	let project: Project | null = null;
	let missing: MissingPlugin[] = [];
	let searching = false;

	function pluginName(project: Project, entry: UnavailablePlugin): string {
		for (const track of project.tracks) {
			const effect = track.effects.find((e) => e.id === entry.instance_key);
			const savedName = (effect?.params as { params?: { name?: string } } | undefined)?.params?.name;
			if (savedName) return savedName;
		}
		return pluginDisplayName(entry.plugin_id);
	}

	function usage(project: Project, instanceKey: string): string {
		if (instanceKey.startsWith(INSTRUMENT_KEY_PREFIX)) {
			const trackId = instanceKey.slice(INSTRUMENT_KEY_PREFIX.length);
			const track = project.tracks.find((t) => t.id === trackId);
			return `Instrument de « ${track?.name ?? 'piste inconnue'} »`;
		}
		const track = project.tracks.find((t) => t.effects.some((e) => e.id === instanceKey));
		return `Effet sur « ${track?.name ?? 'piste inconnue'} »`;
	}

	/** One row per plugin, listing every place it is used. */
	function summarize(project: Project, list: UnavailablePlugin[]): MissingPlugin[] {
		const byPlugin = new Map<string, MissingPlugin>();
		for (const entry of list) {
			const row = byPlugin.get(entry.plugin_id) ?? {
				pluginId: entry.plugin_id,
				name: pluginName(project, entry),
				reason: entry.reason,
				usages: [],
			};
			row.usages.push(usage(project, entry.instance_key));
			byPlugin.set(entry.plugin_id, row);
		}
		return [...byPlugin.values()].sort((a, b) => a.name.localeCompare(b.name));
	}

	async function refresh() {
		if (!project) return;
		const list = await invoke<UnavailablePlugin[]>('get_unavailable_plugins');
		missing = summarize(project, list);
	}

	async function handleSearchAgain() {
		searching = true;
		try {
			await rescanPlugins();
			await refresh();
		} finally {
			searching = false;
		}
	}

	function close() {
		missing = [];
	}

	function handleKey(e: KeyboardEvent) {
		if (missing.length > 0 && e.key === 'Escape') close();
	}

	onMount(() => {
		const unlisten = listen<Project>('project-loaded', async ({ payload }) => {
			project = payload;
			try {
				await refresh();
			} catch (e) {
				console.error('[plugins] get_unavailable_plugins failed:', e);
			}
		});
		return () => {
			unlisten.then((fn) => fn());
		};
	});

	$: count = missing.length;
</script>

<svelte:window on:keydown={handleKey} />

{#if count > 0}
	<div class="backdrop" role="presentation">
		<div class="dialog" role="dialog" aria-modal="true" aria-labelledby="missing-plugins-title">
			<h2 id="missing-plugins-title">
				{count === 1 ? 'Un plugin est indisponible' : `${count} plugins sont indisponibles`}
			</h2>
			<p class="intro">
				Les pistes concernées sont muettes, ou leur son passe sans être traité. Leurs réglages sont
				conservés, même si vous enregistrez. Réinstallez les plugins puis cliquez sur « Rechercher à
				nouveau ».
			</p>

			<ul class="list">
				{#each missing as plugin (plugin.pluginId)}
					<li>
						<div class="row-head">
							<span class="name">{plugin.name}</span>
							<span class="badge">{UNAVAILABLE_REASON_LABEL[plugin.reason]}</span>
						</div>
						{#each plugin.usages as place}
							<span class="usage">{place}</span>
						{/each}
					</li>
				{/each}
			</ul>

			<div class="actions">
				<button class="secondary" type="button" on:click={handleSearchAgain} disabled={searching}>
					{searching ? 'Recherche…' : 'Rechercher à nouveau'}
				</button>
				<button class="primary" type="button" on:click={close}>Continuer</button>
			</div>
		</div>
	</div>
{/if}

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

	.dialog {
		width: 440px;
		max-height: 80vh;
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 16px;
		background: #1e1e26;
		border: 1px solid $border-color;
		border-radius: $radius-lg;
		box-shadow: 0 12px 40px rgba(0, 0, 0, 0.6);
	}

	h2 {
		margin: 0;
		font-size: 14px;
		font-weight: 600;
		color: $text-color;
	}

	.intro {
		margin: 0;
		font-size: 11px;
		line-height: 1.45;
		color: $text-subtle;
	}

	.list {
		margin: 0;
		padding: 0;
		list-style: none;
		overflow-y: auto;
		border: 1px solid $border-faint;
		border-radius: $radius-sm;

		li {
			display: flex;
			flex-direction: column;
			gap: 3px;
			padding: 8px 10px;
			border-bottom: 1px solid $border-faint;
			&:last-child {
				border-bottom: none;
			}
		}
	}

	.row-head {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.name {
		flex: 1;
		min-width: 0;
		font-size: 12px;
		font-weight: 500;
		color: $text-color;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.badge {
		flex-shrink: 0;
		padding: 1px 6px;
		border: 1px solid rgba(255, 65, 54, 0.5);
		border-radius: 3px;
		background: rgba(255, 65, 54, 0.12);
		color: #ff6b61;
		font-size: 9px;
		font-family: $font-mono;
		font-weight: 700;
	}

	.usage {
		font-size: 10px;
		color: $text-faint;
	}

	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
	}

	button {
		padding: 5px 14px;
		border-radius: 4px;
		font-size: 11px;
		cursor: pointer;
	}

	.secondary {
		border: 1px solid $border-subtle;
		background: transparent;
		color: $text-subtle;
		&:hover:not(:disabled) {
			color: $text-color;
			border-color: rgba(21, 113, 255, 0.4);
		}
		&:disabled {
			cursor: default;
			opacity: 0.6;
		}
	}

	.primary {
		border: 1px solid rgba(21, 113, 255, 0.6);
		background: rgba(21, 113, 255, 0.25);
		color: $text-color;
		&:hover {
			background: rgba(21, 113, 255, 0.4);
		}
	}
</style>
