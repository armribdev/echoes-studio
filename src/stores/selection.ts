import { writable } from 'svelte/store';

export interface SelectionState {
	ids: Set<string>;
	/** Last clip clicked — drives the docked piano roll content. */
	primaryId: string | null;
}

function emptyState(): SelectionState {
	return { ids: new Set(), primaryId: null };
}

function sameSelection(a: SelectionState, ids: Set<string>, primaryId: string): boolean {
	if (a.primaryId !== primaryId || a.ids.size !== ids.size) return false;
	for (const id of ids) {
		if (!a.ids.has(id)) return false;
	}
	return true;
}

function createSelectionStore() {
	const { subscribe, set, update } = writable<SelectionState>(emptyState());

	return {
		subscribe,

		select(id: string, options?: { additive?: boolean }) {
			update((s) => {
				const ids = new Set(s.ids);
				if (options?.additive) {
					if (ids.has(id)) ids.delete(id);
					else ids.add(id);
					if (sameSelection(s, ids, id)) return s;
					return { ids, primaryId: id };
				}
				if (!ids.has(id)) {
					ids.clear();
					ids.add(id);
				}
				if (sameSelection(s, ids, id)) return s;
				return { ids, primaryId: id };
			});
		},

		setPrimary(id: string) {
			update((s) => (s.primaryId === id ? s : { ...s, primaryId: id }));
		},

		setSingle(id: string) {
			const next = { ids: new Set([id]), primaryId: id };
			update((s) => (sameSelection(s, next.ids, id) ? s : next));
		},

		/** Remplace un id dans la sélection (ex. pending → clip moteur). */
		remapId(from: string, to: string) {
			if (from === to) return;
			update((s) => {
				if (!s.ids.has(from)) return s;
				const ids = new Set(s.ids);
				ids.delete(from);
				ids.add(to);
				const primaryId = s.primaryId === from ? to : s.primaryId;
				if (sameSelection(s, ids, primaryId ?? to)) return s;
				return { ids, primaryId };
			});
		},

		setMany(ids: string[], options?: { additive?: boolean }) {
			update((s) => {
				const nextIds = options?.additive ? new Set(s.ids) : new Set<string>();
				for (const id of ids) nextIds.add(id);
				if (nextIds.size === 0) {
					return s.ids.size === 0 ? s : emptyState();
				}
				const primaryId = ids.length > 0 ? ids[ids.length - 1]! : s.primaryId ?? [...nextIds][0]!;
				if (sameSelection(s, nextIds, primaryId)) return s;
				return { ids: nextIds, primaryId };
			});
		},

		clear() {
			update((s) => (s.ids.size === 0 ? s : emptyState()));
		},
	};
}

export const selection = createSelectionStore();
