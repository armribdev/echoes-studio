import { writable } from 'svelte/store';

export interface ModulationDrag {
	modulatorId: string;
	color: string;
	/** Target key (`targets.ts` option key) currently under the pointer. */
	hoverKey: string | null;
}

export const modulationDrag = writable<ModulationDrag | null>(null);
