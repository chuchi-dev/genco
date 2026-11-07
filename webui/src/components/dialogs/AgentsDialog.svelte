<script lang="ts">
	import Markdown from '../Markdown.svelte';
	import OneTimeSecret from '../OneTimeSecret.svelte';
	import ProposalText from '../ProposalText.svelte';
	import { formatTime, get, post, type Peer, type Proposal } from '@/lib/api';
	import {
		dialogs,
		message,
		confirmDelete,
		type Auth,
	} from '@/lib/dialogs.svelte';
	import { proposals } from '@/lib/proposals.svelte';
	import { setChat } from '@/lib/router.svelte';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import ListView from '../win/ListView.svelte';
	import Tabs from '../win/Tabs.svelte';
	import ApproveDialog from './ApproveDialog.svelte';
	import RejectDialog from './RejectDialog.svelte';

	// other AI agents that look after the server: one key each (status,
	// databases, proposals), and their webhook gets the reports and the
	// answers to their proposals
	let { onclose }: { onclose: () => void } = $props();

	const TABS = [
		{ id: 'agents', label: 'Agents' },
		{ id: 'proposals', label: 'Proposals' },
	];
	let tab = $state('agents');

	let peers = $state<Peer[]>([]);
	let selected = $state<number | null>(null);
	let selectedProposal = $state<number | null>(null);
	// raw: the text exactly as stored, what the server agent gets;
	// original: what the agent sent, before the rewrite
	let view = $state<'formatted' | 'raw' | 'original'>('formatted');
	// the form: null is the list, 0 a new agent
	let editing = $state<number | null>(null);
	let form = $state({ name: '', webhookUrl: '', webhookAll: false });
	// a key just created, shown once
	let key = $state<string | null>(null);
	let testResult = $state<string | null>(null);
	let error = $state<string | null>(null);
	let approving = $state<Proposal | null>(null);
	let rejecting = $state<Proposal | null>(null);

	const peer = $derived(peers.find(p => p.id === selected));
	const proposal = $derived(
		proposals.list.find(p => p.id === selectedProposal),
	);

	async function load() {
		try {
			[peers] = await Promise.all([get('/peers'), proposals.load()]);
			error = null;
		} catch (e: any) {
			error = e.message;
		}
	}
	// opens on the proposals when one waits (the link of the mail)
	load().then(() => {
		if (proposals.waiting.length) tab = 'proposals';
	});
	// Find opens it on one proposal, also when it is open
	$effect(() => {
		const id = dialogs.proposal;
		if (id === null) return;
		tab = 'proposals';
		selectedProposal = id;
		dialogs.proposal = null;
	});

	function edit(p: Peer | null) {
		editing = p?.id ?? 0;
		form = {
			name: p?.name ?? '',
			webhookUrl: p?.webhookUrl ?? '',
			webhookAll: p?.webhookAll ?? false,
		};
		key = null;
		testResult = null;
	}

	async function save() {
		if (editing) {
			// genco sends the key to the webhook: a new one needs the password
			const old = peers.find(p => p.id === editing)?.webhookUrl;
			let auth: Auth | null = null;
			if (form.webhookUrl.trim() !== old) {
				auth = await message.password(
					'Change Webhook',
					'send the key to another webhook',
				);
				if (auth === null) return;
			}
			await post(`/peers/${editing}`, { ...form, ...auth });
		} else {
			const auth = await message.password(
				'New AI Agent',
				'create a key for an AI agent',
			);
			if (auth === null) return;
			key = await post('/peers', { ...form, ...auth });
		}
		editing = null;
		await load();
	}

	async function newKey() {
		if (!peer) return;
		const ok = await message.confirm(
			'New Key',
			`Make a new key for ${peer.name}? The old one stops working, the agent needs the new one.`,
		);
		if (!ok) return;
		const auth = await message.password('New Key', 'make a new key');
		if (auth === null) return;
		try {
			key = await post(`/peers/${peer.id}/key`, auth);
		} catch (e: any) {
			message.show('No New Key', e.message);
		}
	}

	async function test() {
		if (!peer) return;
		testResult = 'Sending…';
		try {
			await post(`/peers/${peer.id}/test`, {});
			testResult = `The webhook of ${peer.name} took it.`;
		} catch (e: any) {
			testResult = e.message;
		}
	}

	async function remove() {
		const deleted =
			peer &&
			(await confirmDelete({
				title: 'Delete AI Agent',
				text: `Delete ${peer.name}? Its key stops working and it gets no more reports.`,
				path: '/peers/' + peer.id,
			}));
		if (!deleted) return;
		selected = null;
		key = null;
		await load();
	}

	async function approved(conversation: number) {
		approving = null;
		setChat(conversation);
		await load();
	}

	async function rejected() {
		rejecting = null;
		await load();
	}

	const STATUS: Record<Proposal['status'], string> = {
		pending: 'waiting for approval',
		approved: 'the agent works on it',
		answered: 'answered',
		rejected: 'rejected',
		expired: 'expired',
	};

	const auth = $derived(
		`curl -fsS -H "Authorization: Bearer ${key ?? '<key>'}" \\\n `,
	);
	const examples = $derived([
		{
			text: 'Read the status (services, deploys, backups, problems):',
			code: `${auth} ${location.origin}/api/status`,
		},
		{
			text: 'Download a database:',
			code: `${auth} ${location.origin}/api/databases/<name>/dump -o db.sql.gz`,
		},
		{
			text: 'Propose a task to the server agent: "text" says what you want and why, not a long list of steps; a small model rewrites it into a short formatted version (the request may take 30 seconds), the admin approves that version and the server agent gets it; the admin of "approver" gets the mail (left out: every admin), any admin decides; the mail goes after "mailDelay" minutes if nobody decided in the panel (left out: right away):',
			code: `${auth} -H "Content-Type: application/json" \\\n  -d '{"text": "…", "approver": "admin@example.ch", "mailDelay": 10}' \\\n  ${location.origin}/api/proposals`,
		},
		{
			text: 'Its status and answer, or the reason of a rejection (the webhook gets them too, and a message when it expires after 7 days):',
			code: `${auth} ${location.origin}/api/proposals/<id>`,
		},
	]);
</script>

<!-- room to read a proposal -->
<Dialog
	title="AI Agents"
	resizable
	remember="agents"
	icon="agent"
	{onclose}
	width={760}
	height={Math.min(innerHeight * 0.75, 820)}
>
	<Tabs tabs={TABS} selected={tab} onselect={t => (tab = t)}>
		<div class="tab" hidden={tab !== 'agents'}>
			{#if editing !== null}
				<Form
					onsubmit={save}
					oncancel={() => (editing = null)}
					ok={editing ? 'Save' : 'Create'}
				>
					{#if !editing}
						<label for="ag-name">Name:</label>
						<!-- svelte-ignore a11y_autofocus -->
						<input
							id="ag-name"
							autofocus
							placeholder="e.g. Corleone"
							bind:value={form.name}
						/>
					{/if}
					<label for="ag-url">Webhook:</label>
					<input
						id="ag-url"
						type="url"
						placeholder="https://… (empty for none)"
						bind:value={form.webhookUrl}
					/>
					<span></span>
					<label class="check">
						<input type="checkbox" bind:checked={form.webhookAll} />
						Send the OK reports too, not only the ones that need attention
					</label>
				</Form>
				<p class="hint">
					The webhook gets a POST with JSON (<code>server</code>,
					<code>status</code>, <code>kind</code>, <code>title</code>,
					<code>text</code>, <code>url</code>, <code>proposal</code>)
					and the agent's key as <code>Authorization: Bearer</code>:
					reports, and the answer to a proposal, its rejection or
					expiry (kind <code>proposal</code>).
				</p>
			{:else}
				<p>
					An agent's key reads the status, downloads every database
					and proposes tasks to the server agent. It belongs to no
					admin.
				</p>
				<ListView
					items={peers}
					key={p => p.id}
					bind:selected
					ondblclick={edit}
					columns={[
						{ label: 'Name' },
						{ label: 'Webhook' },
						{ label: 'Reports' },
						{ label: 'Last used' },
					]}
					empty="No AI agents yet."
					fill
					minHeight={110}
				>
					{#snippet row(p)}
						<td>{p.name}</td>
						<td>{p.webhookUrl || '(none)'}</td>
						<td
							>{p.webhookUrl
								? p.webhookAll
									? 'all'
									: 'attention'
								: ''}</td
						>
						<td>{p.lastUsed ? formatTime(p.lastUsed) : 'never'}</td>
					{/snippet}
				</ListView>
				{#if error}<p class="error">{error}</p>{/if}
				{#if testResult}<p class="selectable">{testResult}</p>{/if}
				{#if key}
					<OneTimeSecret
						value={key}
						label="The agent's key, copy it now, it is shown only once:"
					/>
					{#each examples as e (e.text)}
						<p>{e.text}</p>
						<pre class="code selectable">{e.code}</pre>
					{/each}
				{/if}
				<div class="row-buttons">
					<Button onclick={() => edit(null)}>New Agent…</Button>
					<Button onclick={() => peer && edit(peer)} disabled={!peer}>
						Edit…
					</Button>
					<Button onclick={newKey} disabled={!peer}>New Key…</Button>
					<Button onclick={test} disabled={!peer?.webhookUrl}
						>Test</Button
					>
					<Button onclick={remove} disabled={!peer}>Delete</Button>
					<span class="gap"></span>
					<Button primary onclick={onclose}>Close</Button>
				</div>
			{/if}
		</div>
		<div class="tab" hidden={tab !== 'proposals'}>
			<ListView
				items={proposals.list}
				key={p => p.id}
				bind:selected={selectedProposal}
				columns={[
					{ label: '#', num: true },
					{ label: 'From' },
					{ label: 'Task' },
					{ label: 'Mailed to' },
					{ label: 'Status' },
					{ label: 'Proposed' },
				]}
				empty="No proposals yet."
				minHeight={110}
				maxHeight={160}
			>
				{#snippet row(p)}
					<td class="num">{p.id}</td>
					<td>{p.from}</td>
					<td class="task">{p.text.split('\n')[0]}</td>
					<td>{p.approver ?? 'all admins'}</td>
					<td>{STATUS[p.status]}</td>
					<td>{formatTime(p.createdAt)}</td>
				{/snippet}
			</ListView>
			{#if proposal}
				<ProposalText {proposal} {view} />
				{#if proposal.status === 'pending'}
					<p class="hint">
						Waits for an admin to approve or reject it{proposal.approverEmail
							? `, the mail went to ${proposal.approverEmail}`
							: ''}.
					</p>
				{/if}
				{#if proposal.decidedBy}
					<p class="hint">
						{proposal.status === 'rejected'
							? 'Rejected'
							: 'Approved'} by
						{proposal.decidedBy}{proposal.comment
							? ' with a change:'
							: '.'}
					</p>
					{#if proposal.comment}
						<div class="text selectable">
							<Markdown text={proposal.comment} />
						</div>
					{/if}
				{/if}
				{#if proposal.status === 'expired'}
					<p class="hint">
						Nobody decided within 7 days, {proposal.from} was told.
					</p>
				{/if}
				{#if proposal.answer}
					<p>
						{proposal.status === 'rejected'
							? `The reason sent to ${proposal.from}:`
							: 'The answer that went back:'}
					</p>
					<div class="text selectable">
						<Markdown text={proposal.answer} />
					</div>
				{/if}
			{:else}
				<p class="hint">
					A proposal waits for an admin. Approved, the server agent
					works on it in a new conversation and its answer goes back
					to the agent.
				</p>
			{/if}
			<div class="row-buttons">
				<Button
					onclick={() => (approving = proposal ?? null)}
					disabled={proposal?.status !== 'pending'}
				>
					Approve…
				</Button>
				<Button
					onclick={() => (rejecting = proposal ?? null)}
					disabled={proposal?.status !== 'pending'}
				>
					Reject…
				</Button>
				<Button
					onclick={() =>
						(view = view === 'raw' ? 'formatted' : 'raw')}
					disabled={!proposal}
				>
					{view === 'raw' ? 'Formatted' : 'Raw Text'}
				</Button>
				<Button
					onclick={() =>
						(view = view === 'original' ? 'formatted' : 'original')}
					disabled={!proposal?.rewrittenBy}
				>
					{view === 'original' ? 'Rewritten' : 'Original'}
				</Button>
				<Button
					onclick={() => proposal && setChat(proposal.conversationId)}
					disabled={!proposal?.conversationId}
				>
					Open Chat
				</Button>
				<span class="gap"></span>
				<Button primary onclick={onclose}>Close</Button>
			</div>
		</div>
	</Tabs>
</Dialog>

{#if approving}
	<ApproveDialog
		proposal={approving}
		onapproved={approved}
		onclose={() => (approving = null)}
	/>
{/if}
{#if rejecting}
	<RejectDialog proposal={rejecting} onclose={rejected} />
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.tab:not([hidden]) {
		flex: 1;
		display: flex;
		flex-direction: column;
		padding: 4px;
		min-height: 300px;
	}

	p {
		margin: 8px 0 4px;
	}

	pre.code {
		max-height: 140px;
		overflow: auto;
		white-space: pre-wrap;
	}

	// the answer, formatted like the proposal
	.text {
		@include sunken;
		flex: 1 1 140px;
		min-height: 80px;
		overflow: auto;
		padding: 4px 8px;
		background: #fff;
	}

	.task {
		max-width: 240px;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.check {
		white-space: normal;
	}
</style>
