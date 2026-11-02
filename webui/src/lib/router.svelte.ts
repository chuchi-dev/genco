import { Router, type Request, type Route } from 'chuchi';
import type { Component } from 'svelte';
import { session } from './session.svelte';

/**
 * The pages of the panel with chuchi's router:
 * `pages/routes.ts` registers them, loaded when opened. A page module may
 * export `loadProps` (its data before it shows), `layout` (the frame
 * around it) and `isPublic` (no login needed).
 */

export type Props = Record<string, any>;

export type PageModule = {
	default: Component<any>;
	layout?: Component<any>;
	// any: each page names the props it takes
	loadProps?: (
		props: any,
		lp: LoadProps,
	) => Promise<Props | void> | Props | void;
	isPublic?: boolean;
};

export type LoadModule = (req: Request) => Promise<PageModule>;

/** What `loadProps` gets besides the props of the route. */
export class LoadProps {
	redirect: string | null = null;

	constructor(readonly req: Request) {}

	/** Opens another page instead of this one. */
	setRedirect(path: string) {
		this.redirect = path;
	}
}

export const router = new Router<PageModule>();

/** the open conversation of the agent stays across pages and reloads */
const CHAT_KEY = 'genco-chat';

/** Where the panel is. */
class Current {
	path = $state('/');
	/** the agent's conversation */
	chat = $state<number | null>(
		Number(localStorage.getItem(CHAT_KEY)) || null,
	);
}

export const current = new Current();

/** Opens a page, the agent's conversation stays. */
export function go(path: string, replace = false) {
	router.open(path, {}, { history: replace ? 'replace' : 'push' });
}

/** Shows another conversation in the agent's window, the page stays. */
export function setChat(id: number | null) {
	if (id === current.chat) return;
	current.chat = id;
	if (id) localStorage.setItem(CHAT_KEY, String(id));
	else localStorage.removeItem(CHAT_KEY);
}

/** A page with its frame and props, what App renders. */
export type Page = {
	layout: Component<any> | null;
	component: Component<any>;
	props: Props;
	/** a new key renders the page new */
	key: string;
};

export const view = $state<{ page: Page | null }>({ page: null });

/** A page in a frame: the layout's props go to the page too. */
function withLayout(layout: LoadModule, page: LoadModule): LoadModule {
	return async req => {
		const [l, p] = await Promise.all([layout(req), page(req)]);
		return {
			isPublic: p.isPublic,
			async loadProps(props, lp) {
				props = {
					...props,
					...((await l.loadProps?.(props, lp)) ?? {}),
				};
				return {
					...props,
					...((await p.loadProps?.(props, lp)) ?? {}),
				};
			},
			layout: l.default,
			default: p.default,
		};
	};
}

/** Pages in one frame. */
export function inLayout(
	layout: LoadModule,
	pages: [string | RegExp, LoadModule][],
) {
	for (const [path, page] of pages)
		router.register(path, withLayout(layout, page));
}

type Fallbacks = {
	notFound: () => Promise<{ default: Component<any> }>;
	/** gets `error` */
	error: () => Promise<{ default: Component<any> }>;
};

let fallbacks: Fallbacks;

/** The page of a request, its data loaded, or where to go instead. */
async function resolve(
	req: Request,
	route: Route<PageModule> | null,
): Promise<{ page: Page } | { redirect: string }> {
	// the conversation in the query is no new page
	const key = req.pathname;
	if (!route) {
		const NotFound = (await fallbacks.notFound()).default;
		return { page: { layout: null, component: NotFound, props: {}, key } };
	}

	try {
		const mod = await route.load(req);
		if (!mod.isPublic && !session.token) {
			const back = new URLSearchParams({ url: req.uri });
			return { redirect: `/login?${back}` };
		}
		const lp = new LoadProps(req);
		let props: Props = route.toProps(req);
		props = { ...props, ...((await mod.loadProps?.(props, lp)) ?? {}) };
		if (lp.redirect) return { redirect: lp.redirect };
		return {
			page: {
				layout: mod.layout ?? null,
				component: mod.default,
				props,
				key,
			},
		};
	} catch (e) {
		const ErrorPage = (await fallbacks.error()).default;
		return {
			page: {
				layout: null,
				component: ErrorPage,
				props: { error: e },
				key,
			},
		};
	}
}

/**
 * Starts the router: `onfirst` is called when the first page is ready, to
 * mount the app.
 */
export function startRouter(pages: Fallbacks, onfirst: () => void) {
	fallbacks = pages;
	router.currentRequest.subscribe(req => {
		if (!req) return;
		current.path = req.pathname || '/';
		// a link of a mail opens a conversation, the address drops it
		const chat = Number(req.url.searchParams.get('chat'));
		if (chat > 0) {
			setChat(chat);
			history.replaceState(history.state, '', current.path);
		}
	});

	let mounted = false;
	router.onRoute(async (req, route, routing) => {
		const r = await resolve(req, route);
		if ('redirect' in r) {
			router.open(r.redirect, {}, { history: 'replace' });
			await routing.dataReady();
			return;
		}
		if (await routing.dataReady()) return;
		view.page = r.page;
		if (!mounted) {
			mounted = true;
			onfirst();
		}
		await Promise.resolve();
		await routing.domReady();
	});
	router.initClient();
}
