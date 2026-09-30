<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { dismissable } from '../frontend/dismiss';
	import { focusTrap } from '../frontend/focusTrap';
	import { appInert, portal } from '../frontend/portal';
	import LibraryBrowser from './LibraryBrowser.svelte';
	import { libraryStore, libraryPopup, defaultSourceForContext } from '../frontend/library';
	import { t } from 'svelte-i18n';

	let rootEl: HTMLDivElement | null = null;
	let adjustedX = 0;
	let adjustedY = 0;

	$: popup = $libraryPopup;

	const POPUP_W = 440;
	const POPUP_H = 560;

	onMount(() => {
		// Context default source (Bitwig); keep the user's query.
		libraryStore.setSource(defaultSourceForContext(popup.context));
		void tick().then(() => {
			if (!rootEl) return;
			const rect = rootEl.getBoundingClientRect();
			const vw = window.innerWidth;
			const vh = window.innerHeight;
			adjustedX = Math.min(popup.anchorX, vw - rect.width - 12);
			adjustedY = Math.min(popup.anchorY, vh - rect.height - 12);
			adjustedX = Math.max(12, adjustedX);
			adjustedY = Math.max(12, adjustedY);
		});
	});

	function close() {
		libraryStore.stopPreview();
		libraryPopup.close();
	}
</script>

<div
	class="library-popup"
	bind:this={rootEl}
	use:portal
	use:appInert
	use:focusTrap={{ initial: 'first' }}
	use:dismissable={close}
	role="dialog"
	aria-modal="true"
	tabindex="-1"
	aria-label={$t('library.aria')}
	style="left: {adjustedX || popup.anchorX}px; top: {adjustedY ||
		popup.anchorY}px; width: {POPUP_W}px; height: min({POPUP_H}px, calc(100vh - 24px))"
>
	<LibraryBrowser
		variant="popup"
		autofocus
		insertTrackId={popup.trackId}
		onInserted={close}
		onRequestClose={close}
	/>
</div>

<style lang="scss">
	@use '../variables.scss' as *;

	.library-popup {
		position: fixed;
		z-index: 220;
		display: flex;
		flex-direction: column;
		border: 1px solid $border-color;
		border-radius: $radius-lg;
		box-shadow:
			0 12px 40px rgba(0, 0, 0, 0.55),
			0 2px 8px rgba(0, 0, 0, 0.3);
		overflow: hidden;
	}
</style>
