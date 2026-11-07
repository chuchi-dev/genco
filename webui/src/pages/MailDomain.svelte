<script lang="ts" module>
	import { objectPage } from '@/lib/router.svelte';
	import { tree } from '@/lib/tree.svelte';

	// received or forwarded, see `counts`
	const COUNT_COLUMNS = [
		{ label: 'Last 30 days', num: true },
		{ label: 'Last mail' },
	];

	const page = objectPage(
		id => tree.mailDomain(id),
		'This mail domain is gone.',
	);
	export const loadProps = page.loadProps;
</script>

<script lang="ts">
	import { untrack } from 'svelte';
	import {
		formatAgo,
		formatTime,
		get,
		newest,
		post,
		type AddressCount,
		type DnsRecord,
		type Mailbox,
		type MailCounts,
	} from '@/lib/api';
	import { mail } from '@/lib/mail.svelte';
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
	import SendOnlyDialog from '@/components/dialogs/SendOnlyDialog.svelte';
	import SendOnlySettings from '@/components/SendOnlySettings.svelte';

	let { id }: { id: string } = $props();

	const live = page.live(() => id);
	const domain = $derived(live.current);

	const group = $derived(tree.group(domain.groupId));

	let selectedBox = $state<number | null>(null);
	let selectedForward = $state<number | null>(null);
	let selectedSendOnly = $state<number | null>(null);
	// a new password hides the settings shown with the old one
	let newPasswords = $state(0);
	// the dialog that is open: a new mailbox, one to change, a forward, a
	// send-only address
	let editing = $state<'new' | Mailbox | 'forward' | 'send-only' | null>(
		null,
	);
	let modeError = $state<string | null>(null);

	let dns = $state<DnsRecord[] | null>(null);
	// the mail server's own name, the first record of the check
	const host = $derived(dns?.find(r => r.kind === 'A')?.name ?? null);

	// mail per address from Postfix's log, again when the addresses change
	let counts = $state<MailCounts | null>(null);
	const loadCounts = newest();
	$effect(() => {
		const path = `/mail-domains/${domain.id}/counts`;
		void [domain.mailboxes, domain.forwards, domain.sendOnlyAddresses];
		untrack(() => loadCounts(get(path), (c: MailCounts) => (counts = c)));
	});
	const byAddress = $derived(
		new Map(
			[
				...(counts?.mailboxes ?? []),
				...(counts?.forwards ?? []),
				...(counts?.sendOnly ?? []),
			].map(c => [c.address, c]),
		),
	);
	const sentBy = $derived(
		new Map((counts?.mailboxesSent ?? []).map(c => [c.address, c.count])),
	);
	const sent = $derived(
		(counts?.sendOnly ?? []).reduce((n, c) => n + c.count, 0),
	);

	const box = $derived(domain.mailboxes.find(b => b.id === selectedBox));
	const forward = $derived(
		domain.forwards.find(f => f.id === selectedForward),
	);

	const sendOnly = $derived(
		domain.sendOnlyAddresses.find(s => s.id === selectedSendOnly),
	);

	async function newSendOnlyPassword() {
		const yes =
			sendOnly &&
			(await message.confirm(
				'New Password',
				`Give ${sendOnly.address} a new password? Apps with the old one can't send until they get the new one.`,
			));
		if (yes) {
			await post(`/send-only/${sendOnly.id}/password`);
			newPasswords++;
		}
	}

	async function removeSendOnly() {
		const deleted =
			sendOnly &&
			(await confirmDelete({
				title: 'Delete Send-only Address',
				text: `Delete ${sendOnly.address}? Apps that send with its login can't send anymore.`,
				path: '/send-only/' + sendOnly.id,
			}));
		if (deleted) selectedSendOnly = null;
	}

	async function switchMode() {
		const sendOnly = !domain.sendOnly;
		const yes = await message.confirm(
			sendOnly ? 'Send Only' : 'Receive Mail',
			sendOnly
				? `Make ${domain.name} send-only? Its inbox is elsewhere: mail to it no longer arrives here, apps still send as it.`
				: `Receive mail for ${domain.name} here? Only when its MX points to this server, otherwise mail from this server to it lands here instead of its real inbox.`,
		);
		if (!yes) return;
		modeError = null;
		try {
			await post(`/mail-domains/${domain.id}/mode`, { sendOnly });
		} catch (e: any) {
			modeError = e.message;
		}
	}

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

{#if mail.off}
	<p class="error">
		Sending is off: mail is off in Start → Settings → Mail. The mailboxes
		stay as they are.
	</p>
{:else if mail.hostingOff}
	<p class="error">
		Mail hosting is off in Start → Settings → Mail: no new mailboxes,
		forwards or send-only addresses. This mail domain stays until you delete
		it.
	</p>
{/if}

<dl class="props">
	<dt>Mail domain:</dt>
	<dd>{domain.name}</dd>
	<dt>Group:</dt>
	<dd>
		{#if group}<a href="/groups/{group.id}">{group.name}</a>{/if}
	</dd>
	<dt>Mail:</dt>
	<dd>
		{domain.sendOnly
			? 'only sent from apps here, the inbox is elsewhere'
			: 'received here'}
		<Button onclick={switchMode}>
			{domain.sendOnly ? 'Receive Mail Here…' : 'Make Send-only…'}
		</Button>
		{#if modeError}<span class="error">{modeError}</span>{/if}
	</dd>
	{#if counts}
		<dt>Last {counts.days} days:</dt>
		<dd>
			{#if domain.sendOnly}
				{sent} sent by send-only addresses
			{:else}
				{counts.received} received, {counts.forwarded} forwarded, {counts.rejected}
				rejected{#if counts.bounced}, {counts.bounced} bounced{/if}
			{/if}{#if counts.login}, {counts.login.count} sent with the group's login
				{counts.login.address}{/if}
			<span class="hint">
				{counts.since
					? `(counted since ${formatTime(counts.since)})`
					: '(counting starts within a minute)'}
			</span>
		</dd>
	{/if}
	{#if host && !domain.sendOnly}
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

{#if !domain.sendOnly}
	<h4>Mailboxes</h4>
	<ListView
		items={domain.mailboxes}
		key={b => b.id}
		bind:selected={selectedBox}
		columns={[
			{ label: 'Address' },
			{ label: 'Quota', num: true },
			...COUNT_COLUMNS,
			{ label: 'Sent (30 days)', num: true },
		]}
		empty="No mailboxes yet."
		ondblclick={b => (editing = b)}
		maxHeight={260}
	>
		{#snippet row(b)}
			<td>{b.address}</td>
			<td class="num">{b.quotaMb} MB</td>
			{@render countCells(byAddress.get(b.address))}
			<td class="num">{sentBy.get(b.address) ?? ''}</td>
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
				requestDelete(
					`/mailboxes/${box.id}`,
					`the mailbox ${box.address}`,
				)}
		>
			Delete…
		</Button>
	</div>

	<h4>Forwards</h4>
	<ListView
		items={domain.forwards}
		key={f => f.id}
		bind:selected={selectedForward}
		columns={[{ label: 'Address' }, { label: 'Goes to' }, ...COUNT_COLUMNS]}
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
			{@render countCells(byAddress.get(f.address))}
		{/snippet}
	</ListView>
	<div class="row-buttons">
		<Button onclick={() => (editing = 'forward')}>New Forward…</Button>
		<Button onclick={removeForward} disabled={!forward}>Delete</Button>
	</div>

	{#if counts?.unknown.length}
		<h4>Unknown addresses (rejected)</h4>
		<p class="hint">
			Mail to addresses without a mailbox or forward, refused in the last
			{counts.days} days: tried by spammers, or one that is missing.
		</p>
		<ListView
			items={counts.unknown}
			key={u => u.address}
			columns={[
				{ label: 'Address' },
				{ label: 'Rejected', num: true },
				{ label: 'Last mail' },
			]}
			maxHeight={200}
		>
			{#snippet row(u)}
				<td>{u.address}</td>
				{@render countCells(u)}
			{/snippet}
		</ListView>
	{/if}
{/if}

<h4>Send-only addresses</h4>
<p class="hint">
	An app's own SMTP login: it sends only as its address, DKIM signed, and has
	no mailbox.
</p>
<ListView
	items={domain.sendOnlyAddresses}
	key={s => s.id}
	bind:selected={selectedSendOnly}
	columns={[
		{ label: 'Address' },
		{ label: 'For' },
		{ label: 'Sent (30 days)', num: true },
		{ label: 'Last mail' },
	]}
	empty="No send-only addresses."
	maxHeight={200}
>
	{#snippet row(s)}
		<td>{s.address}</td>
		<td>{s.note}</td>
		{@render countCells(byAddress.get(s.address))}
	{/snippet}
</ListView>
<div class="row-buttons">
	<Button onclick={() => (editing = 'send-only')}>Add…</Button>
	<Button onclick={newSendOnlyPassword} disabled={!sendOnly}>
		New Password…
	</Button>
	<Button onclick={removeSendOnly} disabled={!sendOnly}>Delete…</Button>
</div>
{#if sendOnly}
	{#key `${sendOnly.id}-${newPasswords}`}
		<SendOnlySettings {sendOnly} />
	{/key}
{/if}

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

{#snippet countCells(c: AddressCount | undefined)}
	<td class="num">{c?.count ?? ''}</td>
	<td>{c ? (c.last ? formatAgo(c.last) : 'never') : ''}</td>
{/snippet}

{#if editing === 'new'}
	<MailboxDialog {domain} onclose={() => (editing = null)} />
{:else if editing === 'send-only'}
	<SendOnlyDialog
		{domain}
		oncreated={id => {
			selectedSendOnly = id;
			editing = null;
		}}
		onclose={() => (editing = null)}
	/>
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
