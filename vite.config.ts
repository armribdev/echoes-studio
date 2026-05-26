import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [sveltekit()],
	css: {
		preprocessorOptions: {
			scss: {
				api: 'modern-compiler'
			}
		}
	},
	server: {
		// Pinning the port and refusing to fall back keeps Vite and Tauri in
		// sync. tauri.conf.json#devPath hardcodes 5173; if Vite ever shifted to
		// 5174 (because 5173 was busy), the Tauri window would open empty with
		// no obvious error. Better to fail fast and tell the user.
		port: 5173,
		strictPort: true,
		watch: {
			// Ignore the Rust source tree so editing a .rs file does not trigger
			// a frontend HMR cycle. Cargo handles its own incremental rebuild.
			ignored: ['**/src-tauri/**']
		}
	}
});
