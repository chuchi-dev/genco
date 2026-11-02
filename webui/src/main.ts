import { mount } from 'svelte';
import App from './App.svelte';
import { startRouter } from './lib/router.svelte';
import { session } from './lib/session.svelte';
import { registerRoutes } from './pages/routes';
import './app.scss';

registerRoutes();
// who is logged in, before the first page
session.init().then(() =>
	startRouter(
		{
			notFound: () => import('./pages/NotFound.svelte'),
			error: () => import('./pages/ErrorPage.svelte'),
		},
		() => mount(App, { target: document.body }),
	),
);
