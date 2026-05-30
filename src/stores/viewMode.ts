import { writable } from 'svelte/store';

export type MainViewMode = 'arrange' | 'graph';
export type GraphScope = 'track' | 'project';

interface ViewModeState {
	main: MainViewMode;
	graphScope: GraphScope;
}

function createViewModeStore() {
	const { subscribe, update } = writable<ViewModeState>({
		main: 'arrange',
		graphScope: 'track',
	});

	return {
		subscribe,
		setMain(main: MainViewMode) {
			update((s) => (s.main === main ? s : { ...s, main }));
		},
		toggleMain() {
			update((s) => ({
				...s,
				main: s.main === 'arrange' ? 'graph' : 'arrange',
			}));
		},
		setGraphScope(scope: GraphScope) {
			update((s) => (s.graphScope === scope ? s : { ...s, graphScope: scope }));
		},
	};
}

export const viewMode = createViewModeStore();
