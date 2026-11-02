import { inLayout, router } from '@/lib/router.svelte';

/** Every page of the panel, the explorer's pages in the desktop. */
export function registerRoutes() {
	router.register('/login', () => import('./Login.svelte'));
	// without the token after the page took it out of the address bar
	router.register(
		/^\/invite(\/(?<token>[0-9a-f]+))?$/,
		() => import('./Invite.svelte'),
	);

	inLayout(
		() => import('./Desktop.svelte'),
		[
			['/', () => import('./Server.svelte')],
			[/^\/clients\/(?<id>\d+)$/, () => import('./Client.svelte')],
			// the open folder of the file manager is part of the path
			[
				/^\/groups\/(?<id>\d+)\/files(?<path>(\/.*)?)$/,
				() => import('./Group.svelte'),
			],
			[
				/^\/groups\/(?<id>\d+)(\/(?<tab>[a-z]+))?$/,
				() => import('./Group.svelte'),
			],
			[/^\/domains\/(?<id>\d+)$/, () => import('./Domain.svelte')],
			[/^\/databases\/(?<id>\d+)$/, () => import('./Database.svelte')],
			[/^\/mail\/(?<id>\d+)$/, () => import('./MailDomain.svelte')],
			[
				/^\/passwords\/(?<id>\d+)$/,
				() => import('./PasswordLink.svelte'),
			],
			['/agents', () => import('./AgentsLink.svelte')],
			[
				/^\/confirm\/(?<token>[0-9a-f]+)$/,
				() => import('./Confirm.svelte'),
			],
		],
	);
}
