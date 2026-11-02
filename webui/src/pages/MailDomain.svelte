<script lang="ts" module>
	import { tree } from '@/lib/tree.svelte';

	export function loadProps({ id }: { id: string }) {
		if (!tree.mailDomain(Number(id)))
			throw new Error('This mail domain is gone.');
	}
</script>

<script lang="ts">
	import { listKeys } from '@/lib/listKeys';
	import { untrack } from 'svelte';
	import { get, newest, type DnsRecord, type Mailbox } from '@/lib/api';
	import {
		message,
		requestDelete,
		confirmDelete,
	} from '@/lib/dialogs.svelte';
	import Button from '@/components/win/Button.svelte';
	import ListView from '@/components/win/ListView.svelte';
	import MailboxDialog from '@/components/dialogs/MailboxDialog.svelte';
	import ForwardDialog from '@/components/dialogs/ForwardDialog.svelte';

	let { id }: { id: string } = $props();

	// what it was when the page opened, a confirmed deletion removes it
	// from the tree while the page still shows
	// svelte-ignore state_referenced_locally
	const start = tree.mailDomain(Number(id))!;
	const domain = $derived(tree.mailDomain(Number(id)) ?? start);
	const group = $derived(tree.group(domain.groupId));

	let selectedBox = $state<number | null>(null);
	let selectedForward = $state<number | null>(null);
	// the dialog that is open: a new mailbox, one to change, a forward
	let editing = $state<'new' | Mailbox | 'forward' | null>(null);

	let dns = $state<DnsRecord[] | null>(null);
	let dnsError = $state<string | null>(null);
	let checking = $state(false);
	// the mail server's own name, the first record of the check
	const host = $derived(dns?.find(r => r.kind === 'A')?.name ?? null);

	const loadDns = newest();
	function check() {
		checking = true;
		loadDns(
			get(`/mail-domains/${domain.id}/dns`),
			(records: DnsRecord[]) => {
				dns = records;
				dnsError = null;
				checking = false;
			},
			e => {
				dnsError = e.message;
				checking = false;
			},
		);
	}
	// again only for another domain: the tree gives a new object on every
	// change anywhere
	const domainId = $derived(Number(id));
	$effect(() => {
		void domainId;
		untrack(check);
	});

	const box = $derived(domain.mailboxes.find(b => b.id === selectedBox));
	const forward = $derived(
		domain.forwards.find(f => f.id === selectedForward),
	);

	async function removeForward() {
		const deleted =
			forward &&
			(await confirmDelete({
				title: 'Delete Forward',
				text: `Delete the forward ${forward.address}?`,
				path: '/forwards/' + forward.id,
			}));
		if (deleted) selectedForward = null;
	}

	const STATUS: Record<DnsRecord['status'], string> = {
		ok: 'ok',
		missing: 'missing',
		differs: 'change',
		optional: 'optional',
	};
</script>

<dl class="props">
	<dt>Mail domain:</dt>
	<dd>{domain.name}</dd>
	<dt>Group:</dt>
	<dd>
		{#if group}<a href="/groups/{group.id}">{group.name}</a>{/if}
	</dd>
	{#if host}
		<dt>Webmail:</dt>
		<dd>
			<a href="https://{host}/" target="_blank" rel="noreferrer">
				https://{host}/
			</a>
			(log in with the whole address)
		</dd>
		<dt>Mail programs:</dt>
		<dd>
			IMAP <code>{host}</code> port 993 (SSL), SMTP <code>{host}</code> port
			465 (SSL) or 587 (STARTTLS); the login is the whole address
		</dd>
	{/if}
</dl>

<h4>Mailboxes</h4>
<ListView
	items={domain.mailboxes}
	key={b => b.id}
	bind:selected={selectedBox}
	columns={[{ label: 'Address' }, { label: 'Quota', num: true }]}
	empty="No mailboxes yet."
	ondblclick={b => (editing = b)}
	maxHeight={260}
>
	{#snippet row(b)}
		<td>{b.address}</td>
		<td class="num">{b.quotaMb} MB</td>
	{/snippet}
</ListView>
<div class="row-buttons">
	<Button onclick={() => (editing = 'new')}>New Mailbox…</Button>
	<Button onclick={() => box && (editing = box)} disabled={!box}>
		Password and Quota…
	</Button>
	<Button
		disabled={!box}
		onclick={() =>
			box &&
			requestDelete(`/mailboxes/${box.id}`, `the mailbox ${box.address}`)}
	>
		Delete…
	</Button>
</div>

<h4>Forwards</h4>
<ListView
	items={domain.forwards}
	key={f => f.id}
	bind:selected={selectedForward}
	columns={[{ label: 'Address' }, { label: 'Goes to' }]}
	empty="No forwards."
	maxHeight={260}
>
	{#snippet row(f)}
		<td>
			{f.address.startsWith('*@')
				? `${f.address} (all others)`
				: f.address}
		</td>
		<td>{f.targets.join(', ')}</td>
	{/snippet}
</ListView>
<div class="row-buttons">
	<Button onclick={() => (editing = 'forward')}>New Forward…</Button>
	<Button onclick={removeForward} disabled={!forward}>Delete</Button>
</div>

<h4>DNS</h4>
<p class="hint">
	Set these at the domain's DNS host. Asking the agent works too, e.g. "what
	do I have to change in the DNS of {domain.name}?"
</p>
{#if dnsError}<p class="error">{dnsError}</p>{/if}
<div class="list-view" {@attach listKeys}>
	<table>
		<thead>
			<tr>
				<th>Status</th>
				<th>Name</th>
				<th>Type</th>
				<th>Value to set</th>
				<th>Now</th>
			</tr>
		</thead>
		<tbody>
			{#each dns ?? [] as r (r.kind + r.name)}
				<tr title={r.note}>
					<td class="dns {r.status}">{STATUS[r.status]}</td>
					<td class="selectable">{r.name}</td>
					<td>{r.kind}</td>
					<td class="selectable mono">{r.value}</td>
					<td class="mono now">{r.found.join(' | ') || '–'}</td>
				</tr>
			{:else}
				<tr><td colspan="5" class="empty">Looking up…</td></tr>
			{/each}
		</tbody>
	</table>
</div>
<div class="row-buttons">
	<Button onclick={check} disabled={checking}>
		{checking ? 'Checking…' : 'Check Again'}
	</Button>
	<span class="gap"></span>
	<Button
		onclick={() =>
			requestDelete(
				`/mail-domains/${domain.id}`,
				`the mail domain ${domain.name}`,
			)}
	>
		Delete Mail Domain…
	</Button>
</div>

{#if editing === 'new'}
	<MailboxDialog {domain} onclose={() => (editing = null)} />
{:else if editing === 'forward'}
	<ForwardDialog {domain} onclose={() => (editing = null)} />
{:else if editing}
	<MailboxDialog
		{domain}
		mailbox={editing}
		onclose={() => (editing = null)}
	/>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	h4 {
		margin: 14px 0 6px;
	}

	// the DNS table, its rows don't select
	.list-view {
		max-height: 260px;
	}

	.mono {
		font-family: $mono;
	}

	.now {
		color: $hint;
		max-width: 260px;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.dns {
		font-weight: bold;

		&.ok {
			color: $ok;
		}

		&.missing,
		&.differs {
			color: $error;
		}

		&.optional {
			color: $hint;
			font-weight: normal;
		}
	}

	.hint {
		margin: 0 0 6px;
	}
</style>
