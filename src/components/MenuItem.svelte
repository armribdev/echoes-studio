<script lang="ts">
	/** Shortcut hint shown on the right. */
	export let shortcut = '';
	export let danger = false;
	export let disabled = false;
	export let title = '';
	/** When set, the item is a checkbox row with a check column. */
	export let checked: boolean | undefined = undefined;
</script>

<button
	class="item"
	class:item--danger={danger}
	type="button"
	role={checked === undefined ? 'menuitem' : 'menuitemcheckbox'}
	aria-checked={checked}
	{disabled}
	{title}
	on:click
>
	{#if $$slots.icon}
		<span class="item-icon"><slot name="icon" /></span>
	{/if}
	{#if checked !== undefined}
		<span class="item-check">
			{#if checked}
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
	{/if}
	<span class="item-label"><slot /></span>
	{#if shortcut}
		<span class="item-shortcut">{shortcut}</span>
	{/if}
</button>

<style lang="scss">
	@use '../variables.scss' as *;
	@use '../controls.scss' as *;

	.item {
		@include list-item;

		&--danger {
			color: $color-error;

			.item-icon {
				color: $color-error;
			}

			&:hover:not(:disabled),
			&:focus-visible {
				background: $error-soft;
				color: $color-error;

				.item-icon {
					color: $color-error;
				}
			}
		}

		&:hover:not(:disabled) .item-icon {
			color: $text-color;
		}

		&:focus-visible .item-icon {
			color: $accent-text;
		}
	}

	.item-icon {
		display: flex;
		flex-shrink: 0;
		color: $text-subtle;
		transition: color $duration-fast;
	}

	.item-check {
		display: flex;
		justify-content: center;
		width: 12px;
		flex-shrink: 0;
		color: $accent-text;
	}

	.item-label {
		min-width: 0;
	}

	.item-shortcut {
		margin-left: auto;
		font-size: 10px;
		color: $text-faint;
		font-family: $font-mono;
		white-space: nowrap;
	}
</style>
