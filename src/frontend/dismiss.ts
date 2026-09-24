import type { ActionReturn } from 'svelte/action';

/**
 * Shared dismiss behaviour for menus, pickers and dialogs.
 *
 * Open layers form a stack: Escape and a pointer press outside only close the
 * topmost one, and the outside press is consumed (like a native menu) so it
 * never also triggers whatever lies underneath.
 */

interface Layer {
	node: HTMLElement;
	dismiss: () => void;
}

const layers: Layer[] = [];

function topLayer(): Layer | undefined {
	return layers[layers.length - 1];
}

/** Called by the global keymap before any other Escape handling. */
export function dismissTopLayer(): boolean {
	const layer = topLayer();
	if (!layer) return false;
	layer.dismiss();
	return true;
}

function consume(e: Event) {
	e.preventDefault();
	e.stopPropagation();
}

function swallowFollowingClick() {
	const swallow = (e: Event) => {
		consume(e);
		stopSwallowing();
	};
	const stopSwallowing = () => {
		window.removeEventListener('click', swallow, true);
		window.removeEventListener('contextmenu', swallow, true);
		window.removeEventListener('pointerdown', stopSwallowing, true);
	};
	window.addEventListener('click', swallow, true);
	window.addEventListener('contextmenu', swallow, true);
	// A press that never produces a click (e.g. a drag) must not eat the next one.
	window.addEventListener('pointerdown', stopSwallowing, true);
}

function handleOutsidePointerDown(e: PointerEvent) {
	const layer = topLayer();
	if (!layer || layer.node.contains(e.target as Node)) return;
	consume(e);
	swallowFollowingClick();
	layer.dismiss();
}

function pushLayer(layer: Layer) {
	if (layers.length === 0) window.addEventListener('pointerdown', handleOutsidePointerDown, true);
	layers.push(layer);
}

function removeLayer(layer: Layer) {
	layers.splice(layers.indexOf(layer), 1);
	if (layers.length === 0) window.removeEventListener('pointerdown', handleOutsidePointerDown, true);
}

/** `use:dismissable={close}` — `close` runs on Escape or a press outside `node`. */
export function dismissable(node: HTMLElement, dismiss: () => void): ActionReturn<() => void> {
	const layer: Layer = { node, dismiss };
	pushLayer(layer);
	return {
		update(nextDismiss) {
			layer.dismiss = nextDismiss;
		},
		destroy() {
			removeLayer(layer);
		}
	};
}
