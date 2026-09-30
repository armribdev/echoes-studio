/**
 * Compat library → arrange. La session réelle vit dans `frontend/audio/fileDrag`
 * (partagée avec le drop OS / explorateur).
 *
 * Note: le drag library utilise un pointer-drag (pas HTML5) parce que
 * `dragDropEnabled` côté Tauri casse le DnD interne WebView2.
 */
export {
	beginAudioFileDrag as beginLibraryDrag,
	endAudioFileDrag as endLibraryDrag,
	getActiveAudioFileDrag,
	isAudioFileDragActive,
	moveAudioFileDragPointer,
	requestAudioFileDragDrop
} from '../audio/fileDrag';

import { getActiveAudioFileDrag } from '../audio/fileDrag';

/** @deprecated Prefer getActiveAudioFileDrag().path */
export function getActiveLibraryDragPath(): string | null {
	return getActiveAudioFileDrag()?.path ?? null;
}
