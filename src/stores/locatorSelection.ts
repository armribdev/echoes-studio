import { writable } from 'svelte/store';

export type LocatorKind = 'marker' | 'region';

export interface LocatorSelection {
	kind: LocatorKind;
	id: string;
}

function createLocatorSelection() {
	const { subscribe, set, update } = writable<LocatorSelection | null>(null);

	return {
		subscribe,
		select: (kind: LocatorKind, id: string) => set({ kind, id }),
		clear: () => set(null),
		toggle: (kind: LocatorKind, id: string) =>
			update((current) =>
				current?.kind === kind && current.id === id ? null : { kind, id },
			),
	};
}

export const locatorSelection = createLocatorSelection();
