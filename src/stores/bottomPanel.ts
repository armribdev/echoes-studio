import { writable } from 'svelte/store';

export type BottomPanelTab = 'pianoroll';

/** Hauteur minimale du panneau en mode aperçu. */
export const PREVIEW_PANEL_MIN = 168;
/** Plafond pour le calcul automatique de hauteur (aperçu). */
export const PREVIEW_PANEL_AUTO_MAX = 600;
/** Plafond pour le redimensionnement manuel en aperçu. */
export const PREVIEW_PANEL_RESIZE_MAX = 480;
/** @deprecated Utiliser PREVIEW_PANEL_MIN */
export const PREVIEW_PANEL_HEIGHT = PREVIEW_PANEL_MIN;
/** @deprecated Utiliser PREVIEW_PANEL_AUTO_MAX */
export const PREVIEW_PANEL_MAX = PREVIEW_PANEL_AUTO_MAX;

/** Default height when entering edit mode for the first time. */
export const DEFAULT_EDIT_PANEL_HEIGHT = 480;

interface BottomPanelState {
	open: boolean;
	activeTab: BottomPanelTab | null;
	height: number;
	savedEditHeight: number;
	/** Hauteur mémorisée du panneau en mode aperçu (redimensionnement manuel). */
	savedPreviewHeight: number;
}

function createBottomPanelStore() {
	const { subscribe, update } = writable<BottomPanelState>({
		open: false,
		activeTab: null,
		height: PREVIEW_PANEL_MIN,
		savedEditHeight: DEFAULT_EDIT_PANEL_HEIGHT,
		savedPreviewHeight: PREVIEW_PANEL_MIN
	});

	function notifyPersist() {
		import('../lib/projectUiLayout').then((m) => m.persistProjectUiLayout());
	}

	return {
		subscribe,
		hydrate(partial: Partial<BottomPanelState>) {
			update((s) => ({ ...s, ...partial }));
		},
		openTab(tab: BottomPanelTab) {
			update((s) => (s.open && s.activeTab === tab ? s : { ...s, open: true, activeTab: tab }));
			notifyPersist();
		},
		toggle() {
			update((s) => ({ ...s, open: !s.open }));
			notifyPersist();
		},
		close() {
			update((s) => ({ ...s, open: false }));
			notifyPersist();
		},
		/** Hauteur auto (ouverture clip / sortie édition). */
		setPreviewAutoHeight(height: number) {
			const clamped = Math.max(PREVIEW_PANEL_MIN, Math.min(PREVIEW_PANEL_AUTO_MAX, height));
			update((s) =>
				s.height === clamped && s.savedPreviewHeight === clamped
					? s
					: { ...s, height: clamped, savedPreviewHeight: clamped }
			);
			notifyPersist();
		},
		/** Redimensionnement manuel en aperçu — persisté pour la session d'aperçu. */
		setPreviewHeight(height: number, maxHeight = PREVIEW_PANEL_RESIZE_MAX) {
			const clamped = Math.max(PREVIEW_PANEL_MIN, Math.min(maxHeight, height));
			update((s) =>
				s.height === clamped && s.savedPreviewHeight === clamped
					? s
					: { ...s, height: clamped, savedPreviewHeight: clamped }
			);
		},
		restorePreviewHeight(maxHeight = PREVIEW_PANEL_RESIZE_MAX) {
			update((s) => {
				const h = Math.max(PREVIEW_PANEL_MIN, Math.min(maxHeight, s.savedPreviewHeight));
				return s.height === h ? s : { ...s, height: h };
			});
		},
		saveEditHeight(h: number) {
			const clamped = Math.max(DEFAULT_EDIT_PANEL_HEIGHT, Math.min(600, h));
			update((s) => (s.savedEditHeight === clamped ? s : { ...s, savedEditHeight: clamped }));
			notifyPersist();
		},
		restoreEditHeight() {
			update((s) => (s.height === s.savedEditHeight ? s : { ...s, height: s.savedEditHeight }));
		},
		/** User resize in edit mode — persisted for next edit session. */
		setEditHeight(h: number) {
			const clamped = Math.max(DEFAULT_EDIT_PANEL_HEIGHT, Math.min(600, h));
			update((s) =>
				s.height === clamped && s.savedEditHeight === clamped
					? s
					: { ...s, height: clamped, savedEditHeight: clamped }
			);
		}
	};
}

export const bottomPanel = createBottomPanelStore();
