import { sveltePreprocess } from 'svelte-preprocess';

/** @type {import('@sveltejs/vite-plugin-svelte').SvelteConfig} */
const config = {
	// makes `@use 'styles/mixins'` work in components
	preprocess: [sveltePreprocess({ scss: { includePaths: ['src'] } })],
	compilerOptions: {
		runes: true,
	},
};

export default config;
