/**
 * Focus trap for dialogs / modals.
 * Stacks traps: only the topmost handles Tab. Portaled menus
 * (`[data-es-focus-trap-portal]`, e.g. Select) stay in the cycle.
 */
import type { Action } from 'svelte/action';

const FOCUSABLE_SELECTOR = [
	'a[href]',
	'button:not([disabled])',
	"input:not([disabled]):not([type='hidden'])",
	'select:not([disabled])',
	'textarea:not([disabled])',
	"[tabindex]:not([tabindex='-1'])"
].join(',');

const PORTAL_SELECTOR = '[data-es-focus-trap-portal]';

export type FocusTrapParams = {
	/** Element focused on activate. Default: the container (tabindex=-1). */
	initial?: 'container' | 'first';
};

const stack: HTMLElement[] = [];
let listening = false;

function isVisible(el: HTMLElement): boolean {
	return el.getClientRects().length > 0;
}

function isInScope(container: HTMLElement, node: Node | null): boolean {
	if (!node) return false;
	if (container.contains(node)) return true;
	if (node instanceof Element && node.closest(PORTAL_SELECTOR)) return true;
	return false;
}

/** Focusables in the container plus options from open portaled menus. */
export function getFocusableElements(container: HTMLElement): HTMLElement[] {
	const seen = new Set<HTMLElement>();
	const out: HTMLElement[] = [];

	const add = (el: HTMLElement) => {
		if (seen.has(el) || !isVisible(el)) return;
		seen.add(el);
		out.push(el);
	};

	for (const el of container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)) {
		add(el);
	}

	// Portals (Select, etc.): include for the top trap. If a confirm is stacked
	// on top, only include a portal that already has focus.
	const top = stack[stack.length - 1];
	if (container === top) {
		const nested = stack.length > 1;
		for (const portal of document.querySelectorAll(PORTAL_SELECTOR)) {
			if (!(portal instanceof HTMLElement)) continue;
			const portalHasFocus =
				document.activeElement instanceof Node && portal.contains(document.activeElement);
			if (nested && !portalHasFocus) continue;
			if (portal.matches(FOCUSABLE_SELECTOR)) add(portal);
			for (const el of portal.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)) {
				add(el);
			}
		}
	}

	return out;
}

function onDocKey(event: KeyboardEvent) {
	if (event.key !== 'Tab') return;
	const container = stack[stack.length - 1];
	if (!container) return;

	const focusables = getFocusableElements(container);
	const active = document.activeElement;

	if (focusables.length === 0) {
		event.preventDefault();
		container.focus({ preventScroll: true });
		return;
	}

	const first = focusables[0]!;
	const last = focusables[focusables.length - 1]!;
	const inScope = isInScope(container, active);

	if (!inScope) {
		event.preventDefault();
		(event.shiftKey ? last : first).focus({ preventScroll: true });
		return;
	}

	if (!event.shiftKey && active === last) {
		event.preventDefault();
		first.focus({ preventScroll: true });
	} else if (event.shiftKey && active === first) {
		event.preventDefault();
		last.focus({ preventScroll: true });
	}
}

function ensureListener() {
	if (listening) return;
	document.addEventListener('keydown', onDocKey, true);
	listening = true;
}

function releaseListener() {
	if (stack.length > 0 || !listening) return;
	document.removeEventListener('keydown', onDocKey, true);
	listening = false;
}

function activate(node: HTMLElement, params: FocusTrapParams | undefined) {
	const previouslyFocused =
		document.activeElement instanceof HTMLElement ? document.activeElement : null;

	stack.push(node);
	ensureListener();

	const initial = params?.initial ?? 'container';
	requestAnimationFrame(() => {
		if (stack[stack.length - 1] !== node) return;
		if (initial === 'first') {
			const focusables = getFocusableElements(node);
			(focusables[0] ?? node).focus({ preventScroll: true });
		} else {
			node.focus({ preventScroll: true });
		}
	});

	return previouslyFocused;
}

function deactivate(node: HTMLElement, previouslyFocused: HTMLElement | null) {
	const idx = stack.lastIndexOf(node);
	if (idx !== -1) stack.splice(idx, 1);
	releaseListener();

	if (
		previouslyFocused &&
		document.contains(previouslyFocused) &&
		typeof previouslyFocused.focus === 'function'
	) {
		previouslyFocused.focus({ preventScroll: true });
	}
}

export const focusTrap: Action<HTMLElement, FocusTrapParams | undefined> = (node, params) => {
	let previouslyFocused = activate(node, params);

	return {
		update(next: FocusTrapParams | undefined) {
			params = next;
		},
		destroy() {
			deactivate(node, previouslyFocused);
		}
	};
};
