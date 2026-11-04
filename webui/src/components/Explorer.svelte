<script lang="ts">
	import type { Snippet } from 'svelte';
	import { mail } from '@/lib/mail.svelte';
	import { current, go } from '@/lib/router.svelte';
	import { tree } from '@/lib/tree.svelte';
	import type { Worker } from '@/lib/api';
	import Menu, { type MenuDef, type MenuEntry } from './win/Menu.svelte';
	import ContextMenu from './win/ContextMenu.svelte';
	import Tree, { type TreeItem } from './win/Tree.svelte';
	import Window from './win/Window.svelte';

	type NewKind = 'client' | 'group' | 'domain' | 'database' | 'mail';

	let {
		onnew,
		onminimize,
		onapply,
		children,
	}: {
		/// opens the dialog to create one, inside the selected item
		onnew: (kind: NewKind) => void;
		onminimize: () => void;
		onapply: () => void;
		/// the page of the selected item
		children: Snippet;
	} = $props();

	// the keys of the tree are the paths of the pages
	const items: TreeItem[] = $derived([
		{
			key: '/',
			label: location.hostname,
			icon: 'computer',
			children: tree.clients.map(c => ({
				key: '/clients/' + c.id,
				label: c.name,
				icon: 'client',
				children: c.groups.map(g => ({
					key: '/groups/' + g.id,
					label: g.name,
					icon: 'group',
					children: [
						...g.domains.map(d => ({
							key: '/domains/' + d.id,
							label: d.name,
							icon: 'domain' as const,
						})),
						...g.databases.map(d => ({
							key: '/databases/' + d.id,
							label: d.name,
							icon: 'database' as const,
						})),
						...g.mailDomains.map(d => ({
							key: '/mail/' + d.id,
							label: d.name,
							icon: 'mail' as const,
						})),
						...g.workers.map(w => ({
							key: `/groups/${g.id}/services/${w.id}`,
							label: w.name + (w.port ? ` :${w.port}` : ''),
							icon: 'gear' as const,
							...workerState(w),
						})),
					],
				})),
			})),
		},
	]);

	// the dot and tooltip of a worker by its unit's state
	function workerState(w: Worker): {
		state: 'ok' | 'bad' | 'off';
		title: string;
	} {
		const [state, text] =
			w.state === 'active'
				? (['ok', 'running'] as const)
				: w.state === 'failed'
					? (['bad', 'failed'] as const)
					: w.state
						? (['off', 'stopped'] as const)
						: (['off', 'not set up yet, Apply Changes'] as const);
		const title = `Worker ${w.name}: ${text}`;
		return { state, title: w.address ? `${title}\n${w.address}` : title };
	}

	// a group's tab is part of its path, a worker's is its own
	const selected = $derived(
		/^\/groups\/\d+\/services\/\d+$/.test(current.path)
			? current.path
			: current.path.split('/').slice(0, 3).join('/') || '/',
	);

	const title = $derived.by(() => {
		const [, kind, idStr] = selected.split('/');
		const id = Number(idStr);
		if (kind === 'clients') return tree.client(id)?.name;
		if (kind === 'groups') return tree.group(id)?.name;
		if (kind === 'domains') return tree.domain(id)?.name;
		if (kind === 'databases') return tree.database(id)?.name;
		if (kind === 'mail') return tree.mailDomain(id)?.name;
		return location.hostname;
	});

	const menus: MenuDef[] = $derived([
		{
			label: 'File',
			entries: [
				{ label: 'New Client…', onclick: () => onnew('client') },
				{
					label: 'New Group…',
					onclick: () => onnew('group'),
					disabled: !tree.clients.length,
				},
				{
					label: 'New Domain…',
					onclick: () => onnew('domain'),
					disabled: !tree.groups.length,
				},
				{
					label: 'New Database…',
					onclick: () => onnew('database'),
					disabled: !tree.groups.length,
				},
				{
					label: 'New Mail Domain…',
					onclick: () => onnew('mail'),
					disabled: !tree.groups.length || mail.hostingOff,
				},
			],
		},
		{
			label: 'Server',
			entries: [{ label: 'Apply Changes…', onclick: () => onapply() }],
		},
		{
			label: 'View',
			entries: [{ label: 'Refresh', onclick: () => tree.load() }],
		},
	]);

	const domains = $derived(tree.groups.flatMap(g => g.domains).length);

	// the right click menu of a tree item; it selects the item first, the
	// New dialogs take their group or client from the selected page
	let context = $state<{ x: number; y: number; entries: MenuEntry[] } | null>(
		null,
	);

	function openContext(key: string, e: MouseEvent) {
		go(key);
		context = { x: e.clientX, y: e.clientY, entries: entriesFor(key) };
	}

	function entriesFor(key: string): MenuEntry[] {
		const [, kind, idStr] = key.split('/');
		const id = Number(idStr);
		const open = { label: 'Open', onclick: () => go(key) };
		const tab = (label: string, t: string) => ({
			label,
			onclick: () => go(`${key}/${t}`),
		});
		const toGroup = (groupId: number | undefined) => ({
			label: 'Open Group',
			onclick: () => go(`/groups/${groupId}`),
			disabled: groupId === undefined,
		});
		const sep = { separator: true as const };
		// a worker: /groups/<id>/services/<worker>
		if (key.split('/').length > 3)
			return [open, sep, toGroup(Number(idStr))];
		switch (kind) {
			case undefined:
			case '':
				return [
					{ label: 'New Client…', onclick: () => onnew('client') },
					sep,
					{ label: 'Apply Changes…', onclick: () => onapply() },
					{ label: 'Refresh', onclick: () => tree.load() },
				];
			case 'clients':
				return [
					open,
					sep,
					{ label: 'New Group…', onclick: () => onnew('group') },
				];
			case 'groups':
				return [
					open,
					sep,
					{ label: 'New Domain…', onclick: () => onnew('domain') },
					{
						label: 'New Database…',
						onclick: () => onnew('database'),
					},
					{
						label: 'New Mail Domain…',
						onclick: () => onnew('mail'),
						disabled: mail.hostingOff,
					},
					sep,
					tab('Files', 'files'),
					tab('Services', 'services'),
					tab('Deploy', 'deploy'),
					tab('Backups', 'backups'),
					tab('Usage', 'usage'),
				];
			case 'domains': {
				const d = tree.domain(id);
				return [
					open,
					{
						label: 'Open Website',
						onclick: () =>
							window.open(
								`https://${d?.name}`,
								'_blank',
								'noreferrer',
							),
						disabled: !d,
					},
					sep,
					toGroup(d?.groupId),
				];
			}
			case 'databases':
				return [open, sep, toGroup(tree.database(id)?.groupId)];
			case 'mail':
				return [open, sep, toGroup(tree.mailDomain(id)?.groupId)];
		}
		return [open];
	}
</script>

<Window title="Explorer - {title ?? ''}" icon="computer" {onminimize}>
	{#snippet menu()}
		<Menu {menus} />
	{/snippet}

	<div class="split">
		<div class="tree">
			<Tree
				{items}
				{selected}
				onselect={key => go(key)}
				oncontext={openContext}
			/>
		</div>
		<div class="detail">
			{@render children()}
		</div>
	</div>

	{#if context}
		<ContextMenu
			x={context.x}
			y={context.y}
			entries={context.entries}
			onclose={() => (context = null)}
		/>
	{/if}

	{#snippet status()}
		<div class="field">{tree.clients.length} client(s)</div>
		<div class="field">{tree.groups.length} group(s)</div>
		<div class="field">{domains} domain(s)</div>
		{#if tree.error}<div class="field error">{tree.error}</div>{/if}
	{/snippet}
</Window>

<style lang="scss">
	@use 'styles/win' as *;

	.split {
		flex: 1;
		display: flex;
		gap: 4px;
		min-height: 0;

		@include mobile {
			flex-direction: column;
		}
	}

	.tree {
		@include sunken;
		width: 240px;
		flex-shrink: 0;
		overflow: auto;
		padding: 4px;

		@include mobile {
			width: auto;
			max-height: 25vh;
		}
	}

	.detail {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		overflow: auto;
		padding: 8px 6px;
	}

	.field {
		@include groove;
		padding: 2px 6px;
		min-width: 90px;

		&.error {
			flex: 1;
			color: $error;
		}
	}
</style>
