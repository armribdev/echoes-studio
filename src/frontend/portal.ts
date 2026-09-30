import type { ActionReturn } from 'svelte/action';

/** Move `node` to `document.body` (modals that must sit above an `inert` shell). */
export function portal(node: HTMLElement): ActionReturn {
	document.body.appendChild(node);
	return {
		destroy() {
			node.remove();
		}
	};
}

let inertDepth = 0;

/** Mark `.app` inert while the action is mounted (ref-counted for stacked modals). */
export function appInert(_node: HTMLElement): ActionReturn {
	const shell = document.querySelector('.app');
	inertDepth += 1;
	shell?.setAttribute('inert', '');
	return {
		destroy() {
			inertDepth = Math.max(0, inertDepth - 1);
			if (inertDepth === 0) shell?.removeAttribute('inert');
		}
	};
}
