<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';
	import { inputMonitorEnabled } from '../stores/audioRecording';
	import { countInEnabled } from '../stores/metronome';
	import { setInputMonitorEnabled } from '../lib/audioRecordingBackend';

	const dispatch = createEventDispatcher<{ close: void }>();

	function toggleMonitor() {
		inputMonitorEnabled.update((enabled) => {
			const next = !enabled;
			setInputMonitorEnabled(next).catch(console.error);
			return next;
		});
	}

	function toggleCountIn() {
		countInEnabled.update((enabled) => !enabled);
	}

	$: items = [
		{
			label: $t('menuBar.monitor'),
			title: $t('menuBar.monitorTitle'),
			checked: $inputMonitorEnabled,
			toggle: toggleMonitor
		},
		{
			label: $t('menuBar.countIn'),
			title: $t('menuBar.countInTitle'),
			checked: $countInEnabled,
			toggle: toggleCountIn
		}
	];
</script>

<div
	class="menu"
	role="menu"
	tabindex="-1"
	use:focusTrap={{ initial: 'first' }}
	use:dismissable={() => dispatch('close')}
>
	{#each items as item (item.label)}
		<button
			class="item"
			type="button"
			role="menuitemcheckbox"
			aria-checked={item.checked}
			title={item.title}
			on:click={item.toggle}
		>
			<span class="check">
				{#if item.checked}
					<svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
						<path
							d="M1.5 5.2L4 7.7L8.5 2.5"
							stroke="currentColor"
							stroke-width="1.4"
							stroke-linecap="round"
							stroke-linejoin="round"
						/>
					</svg>
				{/if}
			</span>
			<span>{item.label}</span>
		</button>
	{/each}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.menu {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		z-index: 1000;
		min-width: 160px;
		padding: 4px;
		background: $panel-background-color;
		border: 1px solid $border-color;
		border-radius: $radius-sm;
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
		user-select: none;
	}

	.item {
		width: 100%;
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px 10px 6px 6px;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: $text-color;
		font-size: 12px;
		font-family: inherit;
		cursor: pointer;
		text-align: left;

		&:hover,
		&:focus-visible {
			background: rgba(21, 113, 255, 0.15);
			outline: none;
		}
	}

	.check {
		width: 12px;
		display: flex;
		justify-content: center;
		color: $accent-color;
	}
</style>
