<script lang="ts" module>
	import { tree } from '@/lib/tree.svelte';

	// the tree of the explorer, loaded once, events keep it fresh
	export async function loadProps() {
		if (!tree.loaded) await tree.load();
	}
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import { post } from '@/lib/api';
	import { listen } from '@/lib/events.svelte';
	import { dialogs } from '@/lib/dialogs.svelte';
	import { track } from '@/lib/drag';
	import { current, go, setChat } from '@/lib/router.svelte';
	import { session } from '@/lib/session.svelte';
	import Agent from '@/components/Agent.svelte';
	import Explorer from '@/components/Explorer.svelte';
	import Taskbar from '@/components/Taskbar.svelte';
	import { proposals } from '@/lib/proposals.svelte';
	import Dialogs from '@/components/Dialogs.svelte';

	/** The explorer around the page of the route. */
	let { children }: { children: Snippet } = $props();

	// a phone shows one window at a time, the agent unless a link opened
	// a page of the explorer (e.g. a confirmation)
	const phone = matchMedia('(max-width: 800px)');
	let mobile = $state(phone.matches);
	$effect(() => {
		const onchange = () => {
			mobile = phone.matches;
			// a wide window has room for both again
			if (!mobile) showAgent = showExplorer = true;
		};
		phone.addEventListener('change', onchange);
		return () => phone.removeEventListener('change', onchange);
	});

	let showAgent = $state(!phone.matches || current.path === '/');
	// the agent's window gets wider or narrower with the bar next to it
	let agentWidth = $state(
		clampWidth(Number(localStorage.getItem('genco-agent-width')) || 420),
	);

	function resizeAgent(e: PointerEvent) {
		const start = agentWidth;
		track(
			e,
			dx => (agentWidth = clampWidth(start + dx)),
			() => localStorage.setItem('genco-agent-width', String(agentWidth)),
		);
	}

	/** the explorer keeps room, also for a width stored on a wider screen */
	function clampWidth(w: number) {
		return Math.min(Math.max(w, 260), innerWidth - 420);
	}

	let showExplorer = $state(!phone.matches || current.path !== '/');

	// a window that got as narrow as a phone keeps one window
	$effect(() => {
		if (mobile && showAgent && showExplorer) showExplorer = false;
	});

	/** a conversation in the agent's window, also when it was hidden */
	function showChat(id: number) {
		if (mobile) toggle(true);
		else showAgent = true;
		setChat(id);
	}

	/** a page in the Explorer, also when it was hidden */
	function showPage(path: string) {
		if (mobile) toggle(false);
		else showExplorer = true;
		go(path);
	}

	// Ctrl+K (⌘K) opens Find from anywhere
	function onkeydown(e: KeyboardEvent) {
		if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
			e.preventDefault();
			dialogs.open = 'find';
		}
	}

	/** the taskbar's button: on a phone it brings the window to the front */
	function toggle(agent: boolean) {
		if (mobile) {
			showAgent = agent;
			showExplorer = !agent;
		} else if (agent) {
			showAgent = !showAgent;
		} else {
			showExplorer = !showExplorer;
		}
	}

	// the taskbar shows the proposals waiting for this admin
	proposals.load().catch(() => {});

	$effect(() =>
		listen(ev => {
			if (ev.type === 'tree' || ev.type === 'hello') tree.load();
			if (ev.type === 'proposals' || ev.type === 'hello')
				proposals.load().catch(() => {});
		}),
	);

	async function logout() {
		try {
			await post('/users/logout');
		} catch (_e) {
			// logged out locally anyway
		}
		session.logout();
		// a fresh start, nothing of the session stays in the page
		window.location.href = '/login';
	}
</script>

<div class="desktop">
	<main>
		<!-- a minimized window is only hidden: the chat keeps its history,
		scroll position and running answer -->
		<div
			class="agent"
			class:hidden={!showAgent}
			style:--width="{agentWidth}px"
		>
			<Agent
				onminimize={() =>
					mobile ? toggle(false) : (showAgent = false)}
			/>
		</div>
		{#if showAgent && showExplorer && !mobile}
			<div
				class="splitter"
				onpointerdown={resizeAgent}
				role="separator"
				aria-orientation="vertical"
				aria-label="Width of the agent's window"
			></div>
		{/if}
		<div class="explorer" class:hidden={!showExplorer}>
			<Explorer
				onnew={k => (dialogs.open = k)}
				onminimize={() =>
					mobile ? toggle(true) : (showExplorer = false)}
				onapply={() => (dialogs.open = 'apply')}
			>
				{@render children()}
			</Explorer>
		</div>
	</main>

	<Taskbar
		onattention={showChat}
		windows={[
			{
				title: 'Server Agent',
				icon: 'agent',
				shown: showAgent,
				ontoggle: () => toggle(true),
			},
			{
				title: 'Explorer',
				icon: 'computer',
				shown: showExplorer,
				ontoggle: () => toggle(false),
			},
		]}
		start={[
			{ label: 'Find…', onclick: () => (dialogs.open = 'find') },
			{ separator: true },
			{ label: 'Settings…', onclick: () => (dialogs.open = 'settings') },
			{ label: 'Users…', onclick: () => (dialogs.open = 'users') },
			{
				label: 'Passwords…',
				onclick: () => (dialogs.open = 'passwords'),
			},
			{ label: 'AI Agents…', onclick: () => (dialogs.open = 'agents') },
			{ label: 'API Tokens…', onclick: () => (dialogs.open = 'tokens') },
			{
				label: 'Activity Log…',
				onclick: () => (dialogs.open = 'activity'),
			},
			{
				label: 'Two-Factor Login…',
				onclick: () => (dialogs.open = 'totp'),
			},
			{
				label: 'Change Email…',
				onclick: () => (dialogs.open = 'account'),
			},
			{
				label: 'Change Password…',
				onclick: () => (dialogs.open = 'password'),
			},
			{ separator: true },
			{
				label: `Log Off ${session.user?.name ?? ''}…`,
				onclick: logout,
			},
		]}
	/>
</div>

<svelte:window {onkeydown} />
<Dialogs onpage={showPage} onchat={showChat} />

<style lang="scss">
	@use 'styles/win' as *;

	.desktop {
		position: fixed;
		inset: 0;
		display: flex;
		flex-direction: column;
	}

	main {
		flex: 1;
		display: flex;
		padding: 6px;
		min-height: 0;

		@include mobile {
			padding: 4px;
		}
	}

	.agent,
	.explorer {
		display: flex;
		min-height: 0;

		&.hidden {
			display: none;
		}

		> :global(.window) {
			flex: 1;
		}
	}

	.agent {
		width: var(--width);
		flex-shrink: 0;

		@include mobile {
			flex: 1;
			width: auto;
			// long tool calls don't make it wider than the screen
			min-width: 0;
		}
	}

	.splitter {
		width: 6px;
		flex-shrink: 0;
		cursor: col-resize;
		touch-action: none;

		@include mobile {
			display: none;
		}
	}

	.explorer {
		flex: 1;
		min-width: 0;

		@include mobile {
			flex: 1;
		}
	}
</style>
