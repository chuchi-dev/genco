<script lang="ts">
	import type { Snippet } from 'svelte';
	import { current, go } from '@/lib/router.svelte';
	import { tree } from '@/lib/tree.svelte';
	import Menu, { type MenuDef } from './win/Menu.svelte';
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
					],
				})),
			})),
		},
	]);

	// a group's tab is part of its path
	const selected = $derived(
		current.path.split('/').slice(0, 3).join('/') || '/',
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
					disabled: !tree.groups.length,
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
</script>

<Window title="Explorer - {title ?? ''}" icon="computer" {onminimize}>
	{#snippet menu()}
		<Menu {menus} />
	{/snippet}

	<div class="split">
		<div class="tree">
			<Tree {items} {selected} onselect={key => go(key)} />
		</div>
		<div class="detail">
			{@render children()}
		</div>
	</div>

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
