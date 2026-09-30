// adapter-static needs `ssr = false` (Tauri runs the bundle as an SPA, no
// server) and a fallback page for client-side routing. `prerender = true`
// generates that fallback at build time without trying to crawl routes.
export const prerender = true;
export const ssr = false;

import '../app.scss';
import { setupI18n } from '../lib/i18n';

setupI18n();
