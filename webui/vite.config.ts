import { resolve } from 'path';
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig(() => {
	return {
		plugins: [svelte()],
		resolve: {
			alias: [{ find: '@', replacement: resolve(__dirname, 'src') }],
		},
		build: {
			// Monaco is one chunk of 2.3 MB, loaded when a file opens
			chunkSizeWarningLimit: 2500,
		},
		server: {
			proxy: { '/api': 'http://127.0.0.1:4100' },
		},
	};
});
