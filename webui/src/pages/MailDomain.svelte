<script lang="ts" module>
	import { GoneError, leaveGone } from '@/lib/router.svelte';
	import { tree } from '@/lib/tree.svelte';

	const GONE = 'This mail domain is gone.';

	export function loadProps({ id }: { id: string }) {
		if (!tree.mailDomain(Number(id))) throw new GoneError(GONE);
	}
</script>

<script lang="ts">
	import { type DnsRecord, type Mailbox } from '@/lib/api';
	import {
		message,
		requestDelete,
		confirmDelete,
	} from '@/lib/dialogs.svelte';
	import Button from '@/components/win/Button.svelte';
	import ListView from '@/components/win/ListView.svelte';
	import MailboxDialog from '@/components/dialogs/MailboxDialog.svelte';
	import DnsTable from '@/components/DnsTable.svelte';
	import ForwardDialog from '@/components/dialogs/ForwardDialog.svelte';

	let { id }: { id: string } = $props();

	// what it was when the page opened, a confirmed deletion removes it
	// from the tree while the page still shows
	// svelte-ignore state_referenced_locally
	const start = tree.mailDomain(Number(id))!;
	const domain = $derived(tree.mailDomain(Number(id)) ?? start);

	// deleted elsewhere: the server's page instead
	$effect(() => {
		if (!tree.mailDomain(Number(id))) leaveGone(GONE);
	});

	const group = $derived(tree.group(domain.groupId));

	let selectedBox = $state<number | null>(null);
	let selectedForward = $state<number | null>(null);
	// the dialog that is open: a new mailbox, one to change, a forward
	let editing = $state<'new' | Mailbox | 'forward' | null>(null);

	let dns = $state<DnsRecord[] | null>(null);
	// the mail server's own name, the first record of the check
	const host = $derived(dns?.find(r => r.kind === 'A')?.name ?? null);

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
<DnsTable path={`/mail-domains/${domain.id}/dns`} bind:records={dns} />
<div class="row-buttons">
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

	.hint {
		margin: 0 0 6px;
	}
</style>
