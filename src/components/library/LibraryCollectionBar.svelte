<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';
	import Button from '../Button.svelte';
	import TextInput from '../TextInput.svelte';
	import { MAX_COLLECTION_NAME_LENGTH } from '../../frontend/library';

	/** The collection currently open, if any. */
	export let collection: { name: string } | null = null;
	/** The search and kind filter can be saved as a new collection. */
	export let canSave = false;
	/** Name used when the field is left empty. */
	export let suggestedName = '';

	const dispatch = createEventDispatcher<{ save: string; remove: void }>();

	let draft = '';

	function submit() {
		dispatch('save', draft.trim() || suggestedName);
		draft = '';
	}
</script>

{#if collection}
	<div class="bar">
		<Button variant="ghost" on:click={() => dispatch('remove')}>
			{$t('library.deleteCollection')}
		</Button>
	</div>
{:else if canSave}
	<form class="bar" on:submit|preventDefault={submit}>
		<TextInput
			bind:value={draft}
			placeholder={suggestedName || $t('library.collectionNamePlaceholder')}
			maxlength={MAX_COLLECTION_NAME_LENGTH}
		/>
		<Button type="submit" compact>{$t('library.saveCollection')}</Button>
	</form>
{/if}

<style lang="scss">
	@use '../../variables.scss' as *;

	.bar {
		flex-shrink: 0;
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: $space-3;
		padding: $space-3 $panel-inset;
		border-bottom: 1px solid $border-color;
	}
</style>
