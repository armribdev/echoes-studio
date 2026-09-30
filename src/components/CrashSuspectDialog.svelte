<!-- CrashSuspectDialog.svelte — after Echoes closed unexpectedly inside a plugin, offer to isolate it -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { invoke } from '@tauri-apps/api/core';
	import { t } from 'svelte-i18n';
	import { preferences } from '../stores/preferences';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';
	import { appInert, portal } from '../frontend/portal';

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
	<div class="backdrop" role="presentation" use:portal use:appInert>
		<div
			class="dialog"
			role="dialog"
			aria-modal="true"
			aria-labelledby="crash-suspect-title"
			tabindex="-1"
			use:focusTrap={{ initial: 'first' }}
			use:dismissable={close}
		>
			<h2 id="crash-suspect-title">
				{$t('crashSuspect.title', { values: { name: suspect.name } })}
			</h2>
			{#if suspect.can_isolate}
				<p class="intro">
					{$t('crashSuspect.introCanIsolate')}
					{#if isolationOff}{$t('crashSuspect.isolationWillEnable')}{/if}
				</p>
				<div class="actions">
					<button class="secondary" type="button" on:click={close}
						>{$t('crashSuspect.later')}</button
					>
					<button class="primary" type="button" on:click={isolate}
						>{$t('crashSuspect.alwaysIsolate', { values: { name: suspect.name } })}</button
					>
				</div>
			{:else}
				<p class="intro">{$t('crashSuspect.introCannotIsolate')}</p>
				<div class="actions">
					<button class="primary" type="button" on:click={close}>{$t('crashSuspect.gotIt')}</button>
				</div>
			{/if}
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
		&:hover {
			color: $text-color;
			border-color: rgba(21, 113, 255, 0.4);
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
