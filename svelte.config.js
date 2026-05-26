import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),

	kit: {
		// adapter-static is the right adapter for Tauri: the frontend ships as a
		// folder of static files that Tauri embeds in the binary. adapter-auto
		// triggers a noisy warning at every `pnpm dev` because no deployment
		// platform is detected (Tauri isn't one).
		//
		// `fallback: 'index.html'` lets SvelteKit work as an SPA — required at
		// runtime since Tauri serves files from a custom protocol, not from a
		// real HTTP server with route resolution.
		adapter: adapter({
			pages: 'build',
			assets: 'build',
			fallback: 'index.html',
			precompress: false,
			strict: true
		})
	}
};

export default config;
