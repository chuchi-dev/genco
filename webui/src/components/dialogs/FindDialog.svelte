<script lang="ts">
	import { get, newest } from '@/lib/api';
	import { conversations } from '@/lib/conversations.svelte';
	import { dialogs } from '@/lib/dialogs.svelte';
	import { tree } from '@/lib/tree.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Icon, { type IconName } from '../win/Icon.svelte';
	import ListView from '../win/ListView.svelte';

	// Start → Find (Ctrl+K): everything with a name, the passwords and the
	// agent's conversations. The tree is searched here, passwords and
	// messages on the server.
	let {
		onclose,
		onpage,
		onchat,
	}: {
		onclose: () => void;
		/** opens a page in the Explorer */
		onpage: (path: string) => void;
		/** opens a conversation in the agent's window */
		onchat: (id: number) => void;
	} = $props();

	type Hit = {
		key: string;
		icon: IconName;
		name: string;
		kind: string;
		/** the group or client it belongs to */
		where: string;
		open: () => void;
	};
	type Secret = {
		id: number;
		name: string;
		username: string;
		url: string;
		note: string;
	};

	let query = $state('');
	let selected = $state<string | null>(null);
	let secrets = $state<Secret[]>([]);
	// conversations whose messages contain the query, from the server
	let found = $state<number[]>([]);

	get('/secrets')
		.then(s => (secrets = s))
		.catch(() => {});

	const q = $derived(query.trim().toLowerCase());
	const has = (...texts: (string | null | undefined)[]) =>
		texts.some(t => t?.toLowerCase().includes(q));

	const search = newest();
	$effect(() => {
		const text = q;
		found = [];
		if (text.length < 3) return;
		const t = setTimeout(() => {
			search(
				get('/conversations/search?q=' + encodeURIComponent(text)),
				(ids: number[]) => (found = ids),
			);
		}, 250);
		return () => clearTimeout(t);
	});

	const hits = $derived.by(() => {
		if (!q) return [];
		const out: Hit[] = [];
		const page = (path: string) => () => {
			onclose();
			onpage(path);
		};

		for (const c of tree.clients) {
			if (has(c.name, c.contact, c.notes))
				out.push({
					key: 'c' + c.id,
					icon: 'client',
					name: c.name,
					kind: 'Client',
					where: '',
					open: page(`/clients/${c.id}`),
				});
			for (const g of c.groups) {
				const where = `${c.name} / ${g.name}`;
				if (has(g.name, g.notes, g.repo))
					out.push({
						key: 'g' + g.id,
						icon: 'group',
						name: g.name,
						kind: 'Group',
						where: c.name,
						open: page(`/groups/${g.id}`),
					});
				for (const d of g.domains)
					if (has(d.name, d.upstream))
						out.push({
							key: 'd' + d.id,
							icon: 'domain',
							name: d.name,
							kind: 'Domain',
							where,
							open: page(`/domains/${d.id}`),
						});
				for (const d of g.databases)
					if (has(d.name))
						out.push({
							key: 'db' + d.id,
							icon: 'database',
							name: d.name,
							kind: 'Database',
							where,
							open: page(`/databases/${d.id}`),
						});
				for (const m of g.mailDomains) {
					if (has(m.name))
						out.push({
							key: 'm' + m.id,
							icon: 'mail',
							name: m.name,
							kind: 'Mail domain',
							where,
							open: page(`/mail/${m.id}`),
						});
					for (const b of m.mailboxes)
						if (has(b.address))
							out.push({
								key: 'mb' + b.id,
								icon: 'mail',
								name: b.address,
								kind: 'Mailbox',
								where,
								open: page(`/mail/${m.id}`),
							});
				}
				for (const w of g.workers)
					if (has(w.name, w.command))
						out.push({
							key: 'w' + w.id,
							icon: 'gear',
							name: w.name,
							kind: 'Worker',
							where,
							open: page(`/groups/${g.id}/services`),
						});
				for (const s of g.schedules)
					if (has(s.name, s.command))
						out.push({
							key: 's' + s.id,
							icon: 'gear',
							name: s.name,
							kind: 'Schedule',
							where,
							open: page(`/groups/${g.id}/services`),
						});
			}
		}

		for (const s of secrets)
			if (has(s.name, s.username, s.url, s.note))
				out.push({
					key: 'p' + s.id,
					icon: 'key',
					name: s.name,
					kind: 'Password',
					where: s.username,
					open: () => {
						dialogs.password = s.id;
						dialogs.open = 'passwords';
					},
				});

		for (const c of conversations.list)
			if (has(c.title) || found.includes(c.id))
				out.push({
					key: 'cv' + c.id,
					icon: 'agent',
					name: c.title,
					kind: 'Conversation',
					where: '',
					open: () => {
						onclose();
						onchat(c.id);
					},
				});
		return out;
	});

	// the first hit is selected while typing, Enter opens it
	$effect(() => {
		if (!hits.some(h => h.key === selected))
			selected = hits[0]?.key ?? null;
	});

	function open(h: Hit | undefined) {
		h?.open();
	}

	// the selected row stays in sight when the arrow keys move it
	$effect(() => {
		void selected;
		document
			.querySelector('.find tr.selected')
			?.scrollIntoView({ block: 'nearest' });
	});

	function onkeydown(e: KeyboardEvent) {
		const at = hits.findIndex(h => h.key === selected);
		if (e.key === 'Enter') {
			e.preventDefault();
			open(hits[at]);
		} else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
			e.preventDefault();
			const next = e.key === 'ArrowDown' ? at + 1 : at - 1;
			selected =
				hits[Math.min(Math.max(next, 0), hits.length - 1)]?.key ?? null;
		}
	}
</script>

<Dialog title="Find" resizable icon="computer" {onclose} width={560}>
	<div class="row">
		<label for="find-q">Find:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			id="find-q"
			type="search"
			autofocus
			autocomplete="off"
			placeholder="domains, groups, databases, passwords, conversations…"
			bind:value={query}
			{onkeydown}
		/>
	</div>
	<ListView
		class="find"
		items={hits}
		key={h => h.key}
		bind:selected
		ondblclick={open}
		columns={[{ label: 'Name' }, { label: 'Type' }, { label: 'In' }]}
		empty={q ? 'Nothing found.' : 'Type to search.'}
		fill
		minHeight={220}
	>
		{#snippet row(h)}
			<td class="name"><Icon name={h.icon} /> {h.name}</td>
			<td>{h.kind}</td>
			<td>{h.where}</td>
		{/snippet}
	</ListView>
	<p class="hint">Enter or a double click opens it. Ctrl+K opens Find.</p>
</Dialog>

<style lang="scss">
	.row {
		display: flex;
		gap: 6px;
		align-items: center;
		margin-bottom: 6px;

		input {
			flex: 1;
		}
	}

	.name {
		display: flex;
		gap: 4px;
		align-items: center;
	}

	p {
		margin: 6px 0 0;
	}
</style>
