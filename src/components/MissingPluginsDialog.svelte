<!-- MissingPluginsDialog.svelte — one summary of unloadable plugins when a project opens -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { listen } from '@tauri-apps/api/event';
	import { invoke } from '@tauri-apps/api/core';
	import { t } from 'svelte-i18n';
	import type { Project } from '../types/global';
	import {
		pluginDisplayName,
		pluginsLoading,
		rescanPlugins,
		type UnavailablePlugin,
		type UnavailableReason
	} from '../stores/plugins';
	import Button from './Button.svelte';
	import Dialog from './Dialog.svelte';
	import DialogFooter from './DialogFooter.svelte';
	import DialogHeader from './DialogHeader.svelte';

	interface MissingPlugin {
		pluginId: string;
		name: string;
		reason: UnavailableReason;
		usages: string[];
	}

	const INSTRUMENT_KEY_PREFIX = 'instrument:';

	const REASON_KEYS: Record<UnavailableReason, string> = {
		not_installed: 'missingPlugins.notInstalled',
		unsupported_format: 'missingPlugins.unsupportedFormat',
		load_failed: 'missingPlugins.loadFailed',
		crashed: 'missingPlugins.crashed'
	};

	let project: Project | null = null;
	let missing: MissingPlugin[] = [];
	let searching = false;

	function pluginName(project: Project, entry: UnavailablePlugin): string {
		for (const track of project.tracks) {
			const effect = track.effects.find((e) => e.id === entry.instance_key);
			const savedName = (effect?.params as { params?: { name?: string } } | undefined)?.params
				?.name;
			if (savedName) return savedName;
		}
		return pluginDisplayName(entry.plugin_id);
	}

	function usage(project: Project, instanceKey: string): string {
		const unknown = get(t)('missingPlugins.unknownTrack');
		if (instanceKey.startsWith(INSTRUMENT_KEY_PREFIX)) {
			const trackId = instanceKey.slice(INSTRUMENT_KEY_PREFIX.length);
			const track = project.tracks.find((t) => t.id === trackId);
			return get(t)('missingPlugins.instrumentOf', {
				values: { name: track?.name ?? unknown }
			});
		}
		const track = project.tracks.find((t) => t.effects.some((e) => e.id === instanceKey));
		return get(t)('missingPlugins.effectOn', { values: { name: track?.name ?? unknown } });
	}

	/** One row per plugin, listing every place it is used. */
	function summarize(project: Project, list: UnavailablePlugin[]): MissingPlugin[] {
		const byPlugin = new Map<string, MissingPlugin>();
		for (const entry of list) {
			const row = byPlugin.get(entry.plugin_id) ?? {
				pluginId: entry.plugin_id,
				name: pluginName(project, entry),
				reason: entry.reason,
				usages: []
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

	onMount(() => {
		const unlisten = listen<Project>('project-loaded', async ({ payload }) => {
			project = payload;
			try {
				if (!get(pluginsLoading)) await refresh();
			} catch (e) {
				console.error('[plugins] get_unavailable_plugins failed:', e);
			}
		});
		return () => {
			unlisten.then((fn) => fn());
		};
	});

	// A plugin still being scanned is not missing: look again when the scan ends.
	$: if (!$pluginsLoading)
		void refresh().catch((e) => console.error('[plugins] refresh failed:', e));

	$: count = missing.length;
</script>

{#if count > 0}
	{@const title =
		count === 1
			? $t('missingPlugins.titleOne')
			: $t('missingPlugins.titleMany', { values: { n: count } })}
	<Dialog label={title} initialFocus="first" on:close={close}>
		<DialogHeader {title} on:close={close} />
		<div class="content">
			<p class="intro">{$t('missingPlugins.intro')}</p>

			<ul class="list">
				{#each missing as plugin (plugin.pluginId)}
					<li>
						<div class="row-head">
							<span class="name">{plugin.name}</span>
							<span class="badge">{$t(REASON_KEYS[plugin.reason])}</span>
						</div>
						{#each plugin.usages as place}
							<span class="usage">{place}</span>
						{/each}
					</li>
				{/each}
			</ul>
		</div>
		<DialogFooter>
			<Button variant="secondary" on:click={handleSearchAgain} disabled={searching}>
				{searching ? $t('missingPlugins.searching') : $t('missingPlugins.searchAgain')}
			</Button>
			<Button variant="primary" on:click={close}>{$t('common.continue')}</Button>
		</DialogFooter>
	</Dialog>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.content {
		display: flex;
		flex-direction: column;
		gap: 10px;
		flex: 1;
		min-height: 0;
		padding: 16px 18px;
	}

	.intro {
		margin: 0;
		font-size: 12px;
		line-height: 1.5;
		color: $text-muted;
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
		border: 1px solid $error-border;
		border-radius: $radius-xs;
		background: $error-soft;
		color: $color-error-text;
		font-size: 9px;
		font-family: $font-mono;
		font-weight: 700;
	}

	.usage {
		font-size: 10px;
		color: $text-faint;
	}
</style>
