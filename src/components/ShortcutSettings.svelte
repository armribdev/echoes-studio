<script lang="ts">
	import { onDestroy } from 'svelte';
	import { t } from 'svelte-i18n';
	import Button from './Button.svelte';
	import SearchInput from './SearchInput.svelte';
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

	<div class="search-row">
		<SearchInput
			bind:value={query}
			label={$t('shortcuts.search')}
			placeholder={$t('shortcuts.searchPlaceholder')}
		/>
	</div>

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
	@use '../controls.scss' as *;

	.shortcuts {
		display: flex;
		flex-direction: column;
		gap: $space-6;
	}

	.shortcuts-header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: $space-5;
	}

	.hint {
		margin: 0;
		font-size: 12px;
		color: $text-muted;
		line-height: 1.4;
	}

	.search-row {
		display: flex;
	}

	.capture-hint {
		margin: 0;
		font-size: 12px;
		color: $accent-color;
	}

	.empty {
		margin: $space-4 0 0;
		font-size: 12px;
		color: $text-muted;
	}

	.category {
		display: flex;
		flex-direction: column;
		gap: $space-3;
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
		gap: $space-1;
	}

	.row {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		grid-template-rows: auto auto;
		align-items: center;
		column-gap: $space-5;
		row-gap: $space-1;
		padding: $space-2 $space-3;
		border-radius: $radius-sm;
		min-height: 32px;

		&.listening {
			background: $accent-wash;
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
		gap: $space-2;
		max-width: 280px;
	}

	.chord {
		@include button($ctrl-height-sm, 11px);
		min-width: 72px;
		font-family: $font-mono;
		font-weight: 400;

		&.custom {
			@include control-on;
		}

		&.listening {
			@include control-on;
			border-color: $accent-color;
			animation: pulse 1.2s ease-in-out infinite;
		}

		&.conflict-chip {
			@include control-on-danger;
		}
	}

	.reset-one {
		@include icon-button($ctrl-height-sm);
		width: auto;
		padding: 0 $space-4;
		font-size: 11px;
		white-space: nowrap;
	}

	.conflict-hint {
		grid-column: 1 / -1;
		font-size: 11px;
		color: $color-error-text;
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
