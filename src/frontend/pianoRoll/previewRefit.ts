import { writable } from 'svelte/store';

/** True pendant un drag de redimensionnement du panneau aperçu. */
export const previewPanelResizing = writable(false);

/** True pendant un drag de redimensionnement du panneau édition. */
export const editPanelResizing = writable(false);
