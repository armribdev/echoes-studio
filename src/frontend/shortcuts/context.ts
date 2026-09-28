/**
 * Keyboard focus contexts — determines which shortcut layer is active.
 *
 * Focus follows the user's last pointer interaction (timeline, piano roll…),
 * not merely whether a panel is open. This lets clip operations work while
 * the piano roll stays visible, as long as the user last clicked the timeline.
 */

import { get } from 'svelte/store';
import { pianoRollStore } from '../../stores/pianoRoll';
import { bottomPanel } from '../../stores/bottomPanel';
import { keyboardFocus, type KeyboardFocusZone } from '../../stores/keyboardFocus';

/** True when the event target is an editable element. */
export function isEditingText(target: EventTarget | null): boolean {
	if (!(target instanceof Element)) return false;
	const tag = target.tagName.toLowerCase();
	return (
		tag === 'input' ||
		tag === 'textarea' ||
		tag === 'select' ||
		(target as HTMLElement).isContentEditable
	);
}

/** True when no modifier keys are held (except shift for some cases). */
export function hasNoModifiers(e: KeyboardEvent): boolean {
	return !e.ctrlKey && !e.metaKey && !e.altKey;
}

/** Piano roll is open and its panel is visible. */
export function isPianoRollOpen(): boolean {
	const pr = get(pianoRollStore);
	const bp = get(bottomPanel);
	return pr.open && bp.open;
}

export function getActiveFocus(): KeyboardFocusZone {
	return get(keyboardFocus);
}

/** Timeline clip shortcuts (delete, cut, copy, paste). */
export function shouldHandleTimelineShortcuts(e: KeyboardEvent): boolean {
	if (isEditingText(e.target)) return false;
	const focus = getActiveFocus();
	if (focus === 'piano-roll' || focus === 'settings') return false;
	if (focus === 'library-results' || focus === 'library-search') return false;
	return true;
}

/** Piano roll editing shortcuts (modes, note delete, selection). */
export function shouldHandlePianoRollShortcuts(e: KeyboardEvent): boolean {
	if (!isPianoRollOpen()) return false;
	if (isEditingText(e.target)) return false;
	return getActiveFocus() === 'piano-roll';
}

/** Global undo/redo — piano roll handles its own history when it has focus. */
export function shouldHandleGlobalHistoryShortcuts(): boolean {
	return getActiveFocus() !== 'piano-roll';
}
