import { get, writable } from 'svelte/store';
import { selectedTrack } from './selectedTrack';

export type MainViewMode = 'arrange' | 'graph';
export type GraphScope = 'track' | 'project';

interface ViewModeState {
	main: MainViewMode;
	graphScope: GraphScope;
}

function graphScopeForSelection(scope: GraphScope): GraphScope {
	return get(selectedTrack).trackId ? scope : 'project';
}

function createViewModeStore() {
	const { subscribe, update } = writable<ViewModeState>({
		main: 'arrange',
		graphScope: 'track',
	});

	selectedTrack.subscribe(() => {
		update((s) => {
			if (s.main !== 'graph' || get(selectedTrack).trackId || s.graphScope === 'project') {
				return s;
			}
			return { ...s, graphScope: 'project' };
		});
	});

	function notifyPersist() {
		import('../lib/projectUiLayout').then((m) => m.persistProjectUiLayout());
	}

	return {
		subscribe,
		hydrate(partial: Partial<ViewModeState>) {
			update((s) => ({ ...s, ...partial }));
		},
		setMain(main: MainViewMode) {
			update((s) => {
				if (s.main === main) return s;
				const graphScope = main === 'graph' ? graphScopeForSelection(s.graphScope) : s.graphScope;
				return { ...s, main, graphScope };
			});
			notifyPersist();
		},
		toggleMain() {
			update((s) => {
				const main = s.main === 'arrange' ? 'graph' : 'arrange';
				const graphScope = main === 'graph' ? graphScopeForSelection(s.graphScope) : s.graphScope;
				return { ...s, main, graphScope };
			});
			notifyPersist();
		},
		setGraphScope(scope: GraphScope) {
			update((s) => {
				const next = s.main === 'graph' ? graphScopeForSelection(scope) : scope;
				return s.graphScope === next ? s : { ...s, graphScope: next };
			});
			notifyPersist();
		},
	};
}

export const viewMode = createViewModeStore();
