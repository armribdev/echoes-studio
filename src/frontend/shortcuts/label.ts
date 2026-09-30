import { derived, get } from 'svelte/store';
import { preferences } from '../../stores/preferences';
import { formatBinding, resolveBinding } from './bindings';
import type { ShortcutId, ShortcutOverrides } from './types';

/** Format the effective binding for menus / tooltips. */
export function shortcutLabel(id: ShortcutId, overrides?: ShortcutOverrides): string {
	const prefs = get(preferences);
	const o = overrides ?? prefs.shortcuts;
	return formatBinding(resolveBinding(id, o), { locale: prefs.interface.locale });
}

/** Reactive label helper — `$shortcutLabels('file.save')`. */
export const shortcutLabels = derived(
	preferences,
	($prefs) => (id: ShortcutId) =>
		formatBinding(resolveBinding(id, $prefs.shortcuts), { locale: $prefs.interface.locale })
);
