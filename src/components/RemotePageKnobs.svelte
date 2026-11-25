<!-- RemotePageKnobs.svelte — the pages of parameters a CLAP plugin offers first, as knobs; nothing while it offers none -->
<script lang="ts">
	import { listen } from '@tauri-apps/api/event';
	import { onDestroy, onMount } from 'svelte';
	import ParamKnob from './ParamKnob.svelte';
	import { unavailablePlugins } from '../stores/plugins';
	import {
		ensurePluginParams,
		fetchPluginParamValues,
		pluginParamText,
		pluginParamsByPlacement,
		setPluginParam
	} from '../stores/pluginParams';
	import { fetchPluginRemotePages, type PluginParamInfo } from '../frontend/modulation/targets';
	import {
		normalisedValue,
		pageLabel,
		pageParams,
		type PluginRemotePage
	} from '../frontend/modulation/remotePages';

	export let instanceKey: string;

	const EDIT_REFRESH_DELAY_MS = 300;
	const SEND_INTERVAL_MS = 30;
	const RETRY_DELAY_MS = 500;
	const MAX_ATTEMPTS = 5;
	/** After a touch on a knob, an edit report is probably its own echo: do not read back. */
	const TOUCH_QUIET_MS = 1000;

	let pages: PluginRemotePage[] = [];
	let pageIndex = 0;
	let values: Record<number, number> = {};
	let texts: Record<number, string> = {};
	let touchedAt = 0;
	let latestRequest = 0;
	let timers: ReturnType<typeof setTimeout>[] = [];
	let sendTimer: ReturnType<typeof setTimeout> | undefined;
	const pending = new Map<number, { info: PluginParamInfo; plain: number }>();

	$: unavailable = $unavailablePlugins.has(instanceKey);
	$: loadPages(instanceKey, unavailable, 1);
	$: page = pages[pageIndex];
	$: params = $pluginParamsByPlacement[instanceKey] ?? [];
	$: infos = page ? pageParams(page, params) : [];
	$: idsKey = infos.map((info) => info.native_id).join();
	$: refreshValues(idsKey);

	onMount(() => {
		ensurePluginParams(instanceKey);
		const unlisten = [
			listen('plugin-state-edited', () => scheduleRefresh()),
			listen('plugin-params-changed', () => loadPages(instanceKey, unavailable, 1))
		];
		return () => unlisten.forEach((stop) => stop.then((fn) => fn()));
	});

	onDestroy(() => {
		timers.forEach(clearTimeout);
		clearTimeout(sendTimer);
	});

	async function loadPages(key: string, isUnavailable: boolean, attempt: number) {
		const request = ++latestRequest;
		const next = isUnavailable ? [] : await fetchPluginRemotePages(key);
		if (request !== latestRequest) return;
		if (next === null) {
			if (attempt < MAX_ATTEMPTS) {
				timers.push(setTimeout(() => loadPages(key, isUnavailable, attempt + 1), RETRY_DELAY_MS));
			}
			return;
		}
		ensurePluginParams(key);
		pages = next;
		pageIndex = Math.min(pageIndex, Math.max(0, next.length - 1));
	}

	function scheduleRefresh() {
		timers.push(
			setTimeout(() => {
				if (Date.now() - touchedAt > TOUCH_QUIET_MS) refreshValues(idsKey);
			}, EDIT_REFRESH_DELAY_MS)
		);
	}

	async function refreshValues(_ids: string) {
		const shown = infos;
		if (shown.length === 0) return;
		const read = await fetchPluginParamValues(
			instanceKey,
			shown.map((info) => info.native_id)
		);
		if (idsKey !== shown.map((info) => info.native_id).join()) return;
		const next = { ...values };
		shown.forEach((info, index) => {
			const plain = read[index];
			if (plain === null || plain === undefined) return;
			next[info.native_id] = plain;
			showText(info, plain);
		});
		values = next;
	}

	function showText(info: PluginParamInfo, plain: number) {
		pluginParamText(instanceKey, info.native_id, plain).then((text) => {
			if (text) texts = { ...texts, [info.native_id]: text };
		});
	}

	function flush() {
		sendTimer = undefined;
		for (const { info, plain } of pending.values()) {
			setPluginParam(instanceKey, info.native_id, normalisedValue(info, plain));
			showText(info, plain);
		}
		pending.clear();
	}

	function change(info: PluginParamInfo, plain: number, immediately: boolean) {
		touchedAt = Date.now();
		values = { ...values, [info.native_id]: plain };
		pending.set(info.native_id, { info, plain });
		if (immediately) {
			clearTimeout(sendTimer);
			flush();
		} else if (sendTimer === undefined) {
			sendTimer = setTimeout(flush, SEND_INTERVAL_MS);
		}
	}

	function step(delta: number) {
		pageIndex = (pageIndex + delta + pages.length) % pages.length;
	}
</script>

{#if page}
	<div class="remote-pages">
		<div class="page-bar">
			{#if pages.length > 1}
				<button type="button" class="step" aria-label="Page précédente" on:click={() => step(-1)}>
					‹
				</button>
			{/if}
			<span class="page-name" title={pageLabel(page)}>{pageLabel(page)}</span>
			{#if pages.length > 1}
				<button type="button" class="step" aria-label="Page suivante" on:click={() => step(1)}>
					›
				</button>
			{/if}
		</div>
		<div class="knobs">
			{#each infos as info (info.native_id)}
				<ParamKnob
					label={info.name}
					value={values[info.native_id] ?? info.default}
					min={info.min}
					max={info.max}
					defaultValue={info.default}
					format={(plain) => texts[info.native_id] ?? plain.toFixed(2)}
					on:change={(e) => change(info, e.detail, false)}
					on:release={(e) => change(info, e.detail, true)}
				/>
			{/each}
		</div>
	</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	.remote-pages {
		display: flex;
		flex-direction: column;
		gap: $space-2;
	}

	.page-bar {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: $space-3;
		min-width: 0;
	}

	.page-name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 9px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: $text-faint;
		user-select: none;
	}

	.step {
		padding: 0 $space-3;
		border: none;
		background: transparent;
		color: $text-subtle;
		font-size: 12px;
		line-height: 1;
		cursor: default;

		&:hover {
			color: $text-color;
		}
	}

	.knobs {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: $space-3 $space-1;
	}
</style>
