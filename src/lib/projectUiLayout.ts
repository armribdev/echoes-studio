import { invoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import {
	bottomPanel,
	DEFAULT_EDIT_PANEL_HEIGHT,
	PREVIEW_PANEL_MIN,
} from '../stores/bottomPanel';
import {
	leftPanel,
	TRACK_INSPECTOR_MAX_WIDTH,
	TRACK_INSPECTOR_MIN_WIDTH,
} from '../stores/leftPanel';
import { rightPanel } from '../stores/rightPanel';
import { viewMode, type GraphScope, type MainViewMode } from '../stores/viewMode';
import type { ProjectUiLayout } from '../types/global';

let hydrating = false;

function clamp(n: number, min: number, max: number): number {
	return Math.max(min, Math.min(max, n));
}

export function collectProjectUiLayout(): ProjectUiLayout {
	const left = get(leftPanel);
	const right = get(rightPanel);
	const bottom = get(bottomPanel);
	const view = get(viewMode);
	return {
		left_panel: {
			open: left.open,
			width: left.width,
		},
		right_panel: {
			open: right.open,
			width: right.width,
		},
		bottom_panel: {
			open: bottom.open,
			height: bottom.height,
			saved_edit_height: bottom.savedEditHeight,
			saved_preview_height: bottom.savedPreviewHeight,
		},
		view_mode: {
			main: view.main,
			graph_scope: view.graphScope,
		},
	};
}

export function applyProjectUiLayout(layout?: ProjectUiLayout | null): void {
	if (!layout) return;
	hydrating = true;
	leftPanel.hydrate({
		open: layout.left_panel.open,
		width: clamp(layout.left_panel.width, TRACK_INSPECTOR_MIN_WIDTH, TRACK_INSPECTOR_MAX_WIDTH),
	});
	rightPanel.hydrate({
		open: layout.right_panel.open,
		width: clamp(layout.right_panel.width, 200, 480),
	});
	bottomPanel.hydrate({
		open: layout.bottom_panel.open,
		height: clamp(layout.bottom_panel.height, PREVIEW_PANEL_MIN, 600),
		savedEditHeight: clamp(
			layout.bottom_panel.saved_edit_height,
			DEFAULT_EDIT_PANEL_HEIGHT,
			600,
		),
		savedPreviewHeight: clamp(layout.bottom_panel.saved_preview_height, PREVIEW_PANEL_MIN, 600),
	});
	const main: MainViewMode =
		layout.view_mode.main === 'graph' ? 'graph' : 'arrange';
	const graphScope: GraphScope =
		layout.view_mode.graph_scope === 'project' ? 'project' : 'track';
	viewMode.hydrate({ main, graphScope });
	hydrating = false;
}

export function persistProjectUiLayout(): void {
	if (hydrating) return;
	void invoke('sync_project_ui_layout', { ui_layout: collectProjectUiLayout() }).catch(
		console.error,
	);
}
