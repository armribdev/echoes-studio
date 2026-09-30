<script lang="ts">
	import { onDestroy } from 'svelte';
	import { t } from 'svelte-i18n';
	import Button from './Button.svelte';
	import {
		chordsForRow,
		settingsCatalog,
		SHORTCUT_CATEGORY_ORDER,
		getShortcutMeta
	} from '../frontend/shortcuts/catalog';
	import {
		bindingFromEvent,
		formatBinding,
		isCustomized,
		resolveBinding,
		rowConflictRoots
	} from '../frontend/shortcuts/bindings';
	import { shortcutRecording } from '../frontend/shortcuts/recording';
	import { preferences } from '../stores/preferences';
	import type {
		KeyBinding,
		ShortcutCategory,
		ShortcutId,
		ShortcutMeta,
		ShortcutOverrides
	} from '../frontend/shortcuts/types';

	export let overrides: ShortcutOverrides = {};

	let recordingId: ShortcutId | null = null;
	let captureHint: string | null = null;
	let query = '';

	$: locale = $preferences.interface.locale;
	$: normalizedQuery = query.trim().toLowerCase();

	$: primaryItems = settingsCatalog();

	$: grouped = SHORTCUT_CATEGORY_ORDER.map((category) => ({
		category,
		items: primaryItems.filter((m) => {
			if (m.category !== category) return false;
			if (!normalizedQuery) return true;
			const hay = `${actionLabel(m.labelKey)} ${categoryLabel(category)}`.toLowerCase();
			return hay.includes(normalizedQuery);
		})
	})).filter((g) => g.items.length > 0);

	function setOverride(id: ShortcutId, binding: KeyBinding | null) {
		const next = { ...overrides };
		if (!binding) {
			delete next[id];
		} else {
			next[id] = binding;
		}
		overrides = next;
	}

	function resetRow(rowId: ShortcutId) {
		const next = { ...overrides };
		for (const chord of chordsForRow(rowId)) {
			delete next[chord.id];
		}
		overrides = next;
		if (recordingId && chordsForRow(rowId).some((c) => c.id === recordingId)) {
			stopRecording();
		}
	}

	function rowIsCustomized(rowId: ShortcutId): boolean {
		return chordsForRow(rowId).some((c) => isCustomized(c.id, overrides));
	}

	function startRecording(id: ShortcutId) {
		captureHint = null;
		if (recordingId) {
			window.removeEventListener('keydown', onCaptureKeyDown, true);
		}
		recordingId = id;
		shortcutRecording.set(true);
		window.addEventListener('keydown', onCaptureKeyDown, true);
	}

	function stopRecording() {
		if (!recordingId) return;
		window.removeEventListener('keydown', onCaptureKeyDown, true);
		recordingId = null;
		shortcutRecording.set(false);
	}

	function onCaptureKey(e: KeyboardEvent) {
		if (!recordingId) return;
		e.preventDefault();
		e.stopImmediatePropagation();

		const result = bindingFromEvent(e);
		if (result === 'incomplete') {
			captureHint = $t('shortcuts.waitForKey');
			return;
		}
		if (result === 'forbidden') {
			captureHint = $t('shortcuts.forbiddenKey');
			return;
		}
		if (result === 'cancel') {
			captureHint = null;
			stopRecording();
			return;
		}

		captureHint = null;
		setOverride(recordingId, result);
		stopRecording();
	}

	function onCaptureKeyDown(e: KeyboardEvent) {
		onCaptureKey(e);
	}

	onDestroy(() => {
		window.removeEventListener('keydown', onCaptureKeyDown, true);
		shortcutRecording.set(false);
	});

	function categoryLabel(category: ShortcutCategory): string {
		return $t(`shortcuts.categories.${category}`);
	}

	function actionLabel(labelKey: string): string {
		return $t(`shortcuts.actions.${labelKey}`);
	}

	function actionLabelForId(id: ShortcutId): string {
		return actionLabel(getShortcutMeta(id).labelKey);
	}

	function conflictText(rowId: ShortcutId): string {
		const roots = rowConflictRoots(rowId, overrides);
		if (roots.length === 0) return '';
		const names = roots.map(actionLabelForId);
		if (names.length === 1) {
			return $t('shortcuts.conflictWith', { values: { action: names[0] } });
		}
		return $t('shortcuts.conflictWithMany', { values: { actions: names.join(', ') } });
	}

	function chordLabel(meta: ShortcutMeta): string {
		return formatBinding(resolveBinding(meta.id, overrides), { locale });
	}

	function resetAll() {
		overrides = {};
		captureHint = null;
		stopRecording();
	}
</script>

<div class="shortcuts">
	<div class="shortcuts-header">
		<p class="hint">{$t('shortcuts.hint')}</p>
		<Button variant="ghost" on:click={resetAll} title={$t('shortcuts.resetAll')}
			>{$t('shortcuts.resetAll')}</Button
		>
	</div>

	<label class="search">
		<span class="sr-only">{$t('shortcuts.search')}</span>
		<input
			type="search"
			bind:value={query}
			placeholder={$t('shortcuts.searchPlaceholder')}
			autocomplete="off"
			spellcheck="false"
		/>
	</label>

	{#if captureHint}
		<p class="capture-hint">{captureHint}</p>
	{/if}

	{#if grouped.length === 0}
		<p class="empty">{$t('shortcuts.noResults')}</p>
	{:else}
		{#each grouped as group}
			<div class="category">
				<h3 class="category-title">{categoryLabel(group.category)}</h3>
				<ul class="list">
					{#each group.items as item}
						{@const chords = chordsForRow(item.id)}
						{@const conflicts = rowConflictRoots(item.id, overrides)}
						{@const customized = rowIsCustomized(item.id)}
						{@const rowListening = chords.some((c) => c.id === recordingId)}
						<li class="row" class:conflict={conflicts.length > 0} class:listening={rowListening}>
							<span class="label">{actionLabel(item.labelKey)}</span>
							<div class="controls">
								{#each chords as chord}
									{@const listening = recordingId === chord.id}
									{@const chipCustom = isCustomized(chord.id, overrides)}
									<button
										type="button"
										class="chord"
										class:custom={chipCustom}
										class:listening
										class:conflict-chip={conflicts.length > 0}
										aria-label={$t('shortcuts.rebindAria', {
											values: { action: actionLabel(item.labelKey) }
										})}
										on:click={() => (listening ? stopRecording() : startRecording(chord.id))}
									>
										{#if listening}
											{$t('shortcuts.pressKey')}
										{:else}
											{chordLabel(chord)}
										{/if}
									</button>
								{/each}
								{#if customized}
									<button
										type="button"
										class="reset-one"
										title={$t('shortcuts.resetOne')}
										aria-label={$t('shortcuts.resetOne')}
										on:click={() => resetRow(item.id)}
									>
										{$t('shortcuts.resetOneShort')}
									</button>
								{/if}
							</div>
							{#if conflicts.length > 0}
								<span class="conflict-hint">{conflictText(item.id)}</span>
							{/if}
						</li>
					{/each}
				</ul>
			</div>
		{/each}
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.shortcuts {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	.shortcuts-header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 12px;
	}

	.hint {
		margin: 0;
		font-size: 12px;
		color: $text-muted;
		line-height: 1.4;
	}

	.search {
		display: block;

		input {
			width: 100%;
			height: $ctrl-height-sm;
			padding: 0 10px;
			font-size: 12px;
			font-family: inherit;
			color: $text-color;
			background: $surface-2;
			border: 1px solid $border-color;
			border-radius: $radius-sm;

			&::placeholder {
				color: $text-subtle;
			}

			&:focus {
				border-color: rgba($accent-color, 0.65);
			}
		}
	}

	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		white-space: nowrap;
		border: 0;
	}

	.capture-hint {
		margin: 0;
		font-size: 12px;
		color: $accent-color;
	}

	.empty {
		margin: 8px 0 0;
		font-size: 12px;
		color: $text-muted;
	}

	.category {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.category-title {
		margin: 0;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		color: $text-muted;
	}

	.list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.row {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		grid-template-rows: auto auto;
		align-items: center;
		column-gap: 12px;
		row-gap: 2px;
		padding: 4px 6px;
		border-radius: $radius-sm;
		min-height: 32px;

		&.listening {
			background: rgba($accent-color, 0.08);
		}
	}

	.label {
		font-size: 13px;
		color: $text-color;
		min-width: 0;
	}

	.controls {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		flex-wrap: wrap;
		gap: 4px;
		max-width: 280px;
	}

	.chord {
		min-width: 72px;
		padding: 4px 8px;
		font-size: 11px;
		font-family: $font-mono;
		color: $text-muted;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid $border-color;
		border-radius: $radius-sm;
		cursor: pointer;
		text-align: center;

		&:hover {
			border-color: rgba(255, 255, 255, 0.2);
			color: $text-color;
		}

		&:focus-visible {
			outline: 2px solid $accent-color;
			outline-offset: 1px;
		}

		&.custom {
			color: $accent-color;
			border-color: rgba($accent-color, 0.45);
		}

		&.listening {
			color: $accent-color;
			border-color: $accent-color;
			animation: pulse 1.2s ease-in-out infinite;
		}

		&.conflict-chip {
			border-color: #e06c75;
			color: #e06c75;
		}
	}

	.reset-one {
		height: 24px;
		padding: 0 6px;
		font-size: 11px;
		line-height: 1;
		color: $text-muted;
		background: transparent;
		border: 1px solid transparent;
		border-radius: $radius-sm;
		cursor: pointer;
		white-space: nowrap;

		&:hover {
			color: $text-color;
			background: rgba(255, 255, 255, 0.06);
			border-color: $border-subtle;
		}

		&:focus-visible {
			outline: 2px solid $accent-color;
			outline-offset: 1px;
		}
	}

	.conflict-hint {
		grid-column: 1 / -1;
		font-size: 11px;
		color: #e06c75;
	}

	@keyframes pulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.55;
		}
	}
</style>
