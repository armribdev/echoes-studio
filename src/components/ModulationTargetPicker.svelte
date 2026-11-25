<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { matchesSearch, type ModulationTargetOption } from '../frontend/modulation/targets';
	import SearchInput from './SearchInput.svelte';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';

	export let options: ModulationTargetOption[];
	export let loading = false;

	const dispatch = createEventDispatcher<{ select: ModulationTargetOption; close: void }>();

	let query = '';

	interface ModuleSection {
		module: string;
		options: ModulationTargetOption[];
	}
	interface GroupSection {
		group: string;
		modules: ModuleSection[];
	}

	$: visible = options.filter((o) => matchesSearch(o, query));
	$: sections = groupOptions(visible);

	function groupOptions(list: ModulationTargetOption[]): GroupSection[] {
		const groups: GroupSection[] = [];
		for (const option of list) {
			let group = groups.find((g) => g.group === option.group);
			if (!group) {
				group = { group: option.group, modules: [] };
				groups.push(group);
			}
			let section = group.modules.find((m) => m.module === option.module);
			if (!section) {
				section = { module: option.module, options: [] };
				group.modules.push(section);
			}
			section.options.push(option);
		}
		return groups;
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && visible.length > 0) dispatch('select', visible[0]);
	}
</script>

<div
	class="picker"
	role="dialog"
	aria-label="Réglage à moduler"
	tabindex="-1"
	use:focusTrap={{ initial: 'first' }}
	use:dismissable={() => dispatch('close')}
>
	<div class="search-row">
		<SearchInput
			placeholder="Rechercher un réglage…"
			bind:value={query}
			on:keydown={handleKeydown}
		/>
	</div>
	<div class="list">
		{#each sections as section (section.group)}
			<div class="group">{section.group}</div>
			{#each section.modules as mod (mod.module)}
				{#if mod.module}<div class="module">{mod.module}</div>{/if}
				{#each mod.options as option (option.key)}
					<button class="option" type="button" on:click={() => dispatch('select', option)}>
						{option.label}
					</button>
				{/each}
			{/each}
		{/each}
		{#if loading}
			<p class="hint">Chargement des réglages des plugins…</p>
		{:else if visible.length === 0}
			<p class="hint">Aucun réglage trouvé</p>
		{/if}
	</div>
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.picker {
		margin: $space-1 $space-3 $space-3;
		border: 1px solid $border-subtle;
		border-radius: $radius-sm;
		background: $background-color;
		overflow: hidden;
	}

	.search-row {
		display: flex;
		padding: $space-3;
		border-bottom: 1px solid $border-faint;
	}

	.list {
		max-height: 240px;
		overflow-y: auto;
	}

	.group {
		padding: $space-3 $space-4 $space-1;
		font-size: 9px;
		font-weight: 700;
		font-family: $font-mono;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: $accent-text;
	}

	.module {
		padding: $space-2 $space-4 1px $space-5;
		font-size: 9px;
		font-family: $font-mono;
		color: $text-faint;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.option {
		@include list-item($ctrl-height-xs, 10px);
		display: block;
		padding: 0 $space-4 0 $space-6;
		border-radius: 0;
		color: $text-subtle;
		line-height: $ctrl-height-xs;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.hint {
		margin: 0;
		padding: $space-4;
		font-size: 9px;
		color: $text-faint;
		text-align: center;
	}
</style>
