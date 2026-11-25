<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';

	export let value = '';
	export let placeholder = '';
	export let label: string | undefined = undefined;
	export let element: HTMLInputElement | undefined = undefined;

	const dispatch = createEventDispatcher<{ clear: void }>();

	function clear() {
		value = '';
		dispatch('clear');
		element?.focus();
	}
</script>

<div class="search">
	<svg
		class="search-icon"
		width="11"
		height="11"
		viewBox="0 0 11 11"
		fill="none"
		aria-hidden="true"
	>
		<circle cx="4.5" cy="4.5" r="3.5" stroke="currentColor" stroke-width="1.2" />
		<path d="M7.5 7.5L10 10" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
	</svg>
	<input
		bind:this={element}
		bind:value
		class="search-input"
		type="text"
		{placeholder}
		aria-label={label ?? placeholder}
		autocomplete="off"
		spellcheck="false"
		on:input
		on:focus
		on:keydown
	/>
	{#if value}
		<button
			class="search-clear"
			type="button"
			tabindex="-1"
			aria-label={$t('common.clear')}
			on:click={clear}
		>
			<svg width="8" height="8" viewBox="0 0 8 8" fill="none" aria-hidden="true">
				<path
					d="M1 1L7 7M7 1L1 7"
					stroke="currentColor"
					stroke-width="1.4"
					stroke-linecap="round"
				/>
			</svg>
		</button>
	{/if}
</div>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.search {
		@include field($ctrl-height-sm, 11px);
		display: flex;
		flex: 1;
		align-items: center;
		gap: $space-3;
		min-width: 0;

		&:has(.search-input:focus-visible) {
			border-color: $accent-color;
			background: $accent-wash;
		}
	}

	.search-icon {
		flex-shrink: 0;
		color: $text-faint;
	}

	.search-input {
		flex: 1;
		min-width: 0;
		padding: 0;
		border: none;
		background: transparent;
		color: $text-color;
		font: inherit;
		outline: none;

		&::placeholder {
			color: $text-faint;
		}
	}

	.search-clear {
		@include icon-button(16px);
		border-radius: $radius-sm;
		color: $text-faint;
	}
</style>
