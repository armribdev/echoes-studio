import { addMessages, init, locale as localeStore } from 'svelte-i18n';
import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import en from '../../locales/en.json';
import fr from '../../locales/fr.json';

export const APP_LOCALES = ['en', 'fr'] as const;
export type AppLocale = (typeof APP_LOCALES)[number];

const STORAGE_KEY = 'echoes-studio-preferences';

export function isAppLocale(value: unknown): value is AppLocale {
	return value === 'en' || value === 'fr';
}

/** Map browser / OS tags (`fr-FR`, `en-US`) to a supported app locale. */
export function resolveLocale(raw: string | null | undefined): AppLocale {
	if (!raw) return 'en';
	const base = raw.toLowerCase().split('-')[0];
	return base === 'fr' ? 'fr' : 'en';
}

function localeFromStorage(): AppLocale | null {
	try {
		if (typeof localStorage === 'undefined') return null;
		const raw = localStorage.getItem(STORAGE_KEY);
		if (!raw) return null;
		const parsed = JSON.parse(raw) as { interface?: { locale?: unknown } };
		return isAppLocale(parsed?.interface?.locale) ? parsed.interface.locale : null;
	} catch {
		return null;
	}
}

function detectInitialLocale(): AppLocale {
	const saved = localeFromStorage();
	if (saved) return saved;
	const nav =
		typeof navigator !== 'undefined' ? (navigator.language ?? navigator.languages?.[0]) : null;
	return resolveLocale(nav);
}

let ready = false;

/** Sync dictionaries + initial locale. Safe to call once at app boot (SPA). */
export function setupI18n(initial?: AppLocale): void {
	if (ready) {
		if (initial) setAppLocale(initial);
		return;
	}
	addMessages('en', en);
	addMessages('fr', fr);
	init({
		fallbackLocale: 'en',
		initialLocale: initial ?? detectInitialLocale()
	});
	ready = true;
	const current = get(localeStore);
	if (isAppLocale(current)) {
		void invoke('set_ui_locale', { locale: current }).catch(() => {});
	}
}

export function setAppLocale(next: AppLocale): void {
	if (!ready) setupI18n(next);
	else localeStore.set(next);
	void invoke('set_ui_locale', { locale: next }).catch(() => {
		/* menu may not be ready in web-only preview */
	});
}

export function getAppLocale(): AppLocale {
	const current = get(localeStore);
	return isAppLocale(current) ? current : 'en';
}

export { localeStore as locale };
