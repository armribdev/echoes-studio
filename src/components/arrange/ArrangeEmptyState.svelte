<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { t } from 'svelte-i18n';
	import Button from '../Button.svelte';

	const dispatch = createEventDispatcher<{
		createTrack: void;
		insertSound: { x: number; y: number };
	}>();
</script>

<div class="empty-tracks">
	<svg
		class="empty-tracks-icon"
		width="32"
		height="32"
		viewBox="0 0 24 24"
		fill="none"
		stroke="currentColor"
		stroke-width="1.5"
		stroke-linecap="round"
		stroke-linejoin="round"
	>
		<path d="M9 18V5l12-2v13" />
		<circle cx="6" cy="18" r="3" />
		<circle cx="18" cy="16" r="3" />
	</svg>
	<p class="empty-tracks-text">{$t('mainPanel.emptyTracks')}</p>
	<p class="empty-tracks-hint">
		Créez une piste, glissez un fichier audio, ou insérez depuis la bibliothèque
	</p>
	<div class="empty-tracks-actions">
		<Button variant="primary" on:click={() => dispatch('createTrack')}>+ Nouvelle piste</Button>
		<Button on:click={(e) => dispatch('insertSound', { x: e.clientX, y: e.clientY })}>
			{$t('trackMenu.insertSound')}
		</Button>
	</div>
</div>

<style lang="scss">
	@use '../../variables.scss' as *;

	.empty-tracks {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		pointer-events: none;

		.empty-tracks-icon {
			color: $text-faint;
			margin-bottom: 4px;
		}

		.empty-tracks-text {
			font-size: 13px;
			font-weight: 500;
			color: $text-muted;
			margin: 0;
		}

		.empty-tracks-hint {
			font-size: 11px;
			color: $text-faint;
			margin: 0;
			text-align: center;
			max-width: 280px;
		}

		.empty-tracks-actions {
			display: flex;
			gap: 8px;
			margin-top: 8px;
			pointer-events: all;
		}
	}
</style>
