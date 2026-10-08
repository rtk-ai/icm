import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [tailwindcss(), sveltekit()],
	server: {
		proxy: {
			'/api': 'http://127.0.0.1:8420',
			// Not '/health' — that's this app's own page route
			// (routes/health/+page.svelte); proxying it to the backend
			// would shadow the page in dev the same way axum's exact-route
			// matching does in production (see web.rs's route comment).
			'/healthz': 'http://127.0.0.1:8420'
		}
	}
});
