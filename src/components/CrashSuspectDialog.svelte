<!-- CrashSuspectDialog.svelte — after Echoes closed unexpectedly inside a plugin, offer to isolate it -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { invoke } from '@tauri-apps/api/core';
	import { t } from 'svelte-i18n';
	import { preferences } from '../stores/preferences';
	import Button from './Button.svelte';
	import Dialog from './Dialog.svelte';
	import DialogFooter from './DialogFooter.svelte';
	import DialogHeader from './DialogHeader.svelte';

	interface CrashSuspect {
		plugin_id: string;
		name: string;
		can_isolate: boolean;
	}

	let suspect: CrashSuspect | null = null;

	function alreadyIsolated(pluginId: string): boolean {
		const { pluginIsolation, alwaysIsolated } = get(preferences).audio;
		return (
			pluginIsolation === 'all' ||
			(pluginIsolation === 'chosen' && alwaysIsolated.includes(pluginId))
		);
	}

	onMount(async () => {
		try {
			const found = await invoke<CrashSuspect | null>('take_crash_suspect');
			if (found && !(found.can_isolate && alreadyIsolated(found.plugin_id))) suspect = found;
		} catch (e) {
			console.error('[plugins] take_crash_suspect failed:', e);
		}
	});

	$: isolationOff = $preferences.audio.pluginIsolation === 'off';

	function isolate() {
		if (!suspect) return;
		const pluginId = suspect.plugin_id;
		preferences.update((prefs) => ({
			...prefs,
			audio: {
				...prefs.audio,
				pluginIsolation: 'chosen',
				alwaysIsolated: [...prefs.audio.alwaysIsolated.filter((id) => id !== pluginId), pluginId]
			}
		}));
		close();
	}

	function close() {
		suspect = null;
	}
</script>

{#if suspect}
	{@const title = $t('crashSuspect.title', { values: { name: suspect.name } })}
	<Dialog label={title} initialFocus="first" on:close={close}>
		<DialogHeader {title} on:close={close} />
		<p class="intro">
			{#if suspect.can_isolate}
				{$t('crashSuspect.introCanIsolate')}
				{#if isolationOff}{$t('crashSuspect.isolationWillEnable')}{/if}
			{:else}
				{$t('crashSuspect.introCannotIsolate')}
			{/if}
		</p>
		<DialogFooter>
			{#if suspect.can_isolate}
				<Button variant="secondary" on:click={close}>{$t('crashSuspect.later')}</Button>
				<Button variant="primary" on:click={isolate}
					>{$t('crashSuspect.alwaysIsolate', { values: { name: suspect.name } })}</Button
				>
			{:else}
				<Button variant="primary" on:click={close}>{$t('crashSuspect.gotIt')}</Button>
			{/if}
		</DialogFooter>
	</Dialog>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.intro {
		margin: 0;
		padding: $space-6;
		font-size: 12px;
		line-height: 1.5;
		color: $text-muted;
	}
</style>
