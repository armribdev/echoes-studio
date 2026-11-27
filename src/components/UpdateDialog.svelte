<!-- UpdateDialog.svelte — offers a newer release found at startup; installing closes the app -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { invoke } from '@tauri-apps/api/core';
	import { check, type Update } from '@tauri-apps/plugin-updater';
	import { t } from 'svelte-i18n';
	import Button from './Button.svelte';
	import Dialog from './Dialog.svelte';
	import DialogFooter from './DialogFooter.svelte';
	import DialogHeader from './DialogHeader.svelte';

	let update: Update | null = null;
	let installing = false;
	let error: string | null = null;

	onMount(async () => {
		if (import.meta.env.DEV) return;
		try {
			update = await check();
		} catch (e) {
			console.warn('[updater] check failed:', e);
		}
	});

	async function install() {
		if (!update) return;
		error = null;
		if (await invoke<boolean>('has_unsaved_changes')) {
			error = $t('update.unsavedChanges');
			return;
		}
		installing = true;
		try {
			await update.downloadAndInstall();
		} catch (e) {
			console.error('[updater] install failed:', e);
			error = $t('update.failed');
			installing = false;
		}
	}

	function close() {
		if (!installing) update = null;
	}
</script>

{#if update}
	{@const title = $t('update.title', { values: { version: update.version } })}
	<Dialog label={title} initialFocus="first" on:close={close}>
		<DialogHeader {title} on:close={close} />
		<div class="content">
			<p class="intro">{$t('update.intro')}</p>
			{#if update.body}
				<p class="notes">{update.body}</p>
			{/if}
			{#if error}
				<p class="error">{error}</p>
			{/if}
		</div>
		<DialogFooter>
			<Button variant="secondary" on:click={close} disabled={installing}
				>{$t('update.later')}</Button
			>
			<Button variant="primary" on:click={install} disabled={installing}>
				{installing ? $t('update.installing') : $t('update.install')}
			</Button>
		</DialogFooter>
	</Dialog>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.content {
		display: flex;
		flex-direction: column;
		gap: $space-5;
		padding: $space-6;
		font-size: 12px;
		line-height: 1.5;

		p {
			margin: 0;
		}
	}

	.intro {
		color: $text-muted;
	}

	.notes {
		white-space: pre-wrap;
		color: $text-color;
	}

	.error {
		color: $color-error-text;
	}
</style>
