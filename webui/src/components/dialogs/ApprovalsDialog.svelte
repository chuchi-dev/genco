<script lang="ts">
	import ActionDetail from '../ActionDetail.svelte';
	import ProposalDetail from '../ProposalDetail.svelte';
	import { formatTime, type Approval } from '@/lib/api';
	import {
		approvals,
		approvalKey,
		approvalKind,
		approvalNumber,
		approvalTitle,
		canConfirm as confirmable,
		createdAt,
		expiresAt,
		requestedBy,
		status,
		statusText,
	} from '@/lib/approvals.svelte';
	import { conversations } from '@/lib/conversations.svelte';
	import { dialogs } from '@/lib/dialogs.svelte';
	import { mail } from '@/lib/mail.svelte';
	import { setChat } from '@/lib/router.svelte';
	import { session } from '@/lib/session.svelte';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';
	import ListView from '../win/ListView.svelte';
	import ApproveDialog from './ApproveDialog.svelte';
	import ConfirmActionDialog from './ConfirmActionDialog.svelte';
	import RejectDialog from './RejectDialog.svelte';

	// Start → Approvals: the AI agents' proposals (#12) and the pending
	// actions (A12: deletions, the agent's remote backup changes) that wait
	// for an admin, the decided ones of 30 days below. Confirming here
	// takes the password and 2FA code; without 2FA only the mail's link
	// confirms, rejecting works here for any admin.
	let { onclose }: { onclose: () => void } = $props();

	let selected = $state<string | null>(null);
	// raw: the text exactly as stored, what the server agent gets;
	// original: what the agent sent, before the rewrite
	let view = $state<'formatted' | 'raw' | 'original'>('formatted');
	let error = $state<string | null>(null);
	let confirming = $state<Approval | null>(null);
	let rejecting = $state<Approval | null>(null);

	const waiting = $derived(approvals.waiting);
	const history = $derived(
		approvals.list.filter(a => status(a) !== 'pending'),
	);
	const item = $derived(
		approvals.list.find(a => approvalKey(a) === selected),
	);
	const proposal = $derived(item?.kind === 'proposal' ? item.proposal : null);
	const action = $derived(item?.kind === 'action' ? item.action : null);
	const pending = $derived(item ? status(item) === 'pending' : false);
	// without 2FA the mail's link confirms; with no mail the password does
	const canConfirm = $derived(
		!!item && confirmable(item, !!session.user?.totp, mail.off),
	);

	async function load() {
		try {
			await Promise.all([approvals.load(), mail.load()]);
			error = null;
		} catch (e: any) {
			error = e.message;
		}
	}
	load();
	// Find and the taskbar open it on one, also when it is open
	$effect(() => {
		const key = dialogs.approval;
		if (key === null) return;
		selected = key;
		view = 'formatted';
		dialogs.approval = null;
	});

	const title = (id: number) =>
		conversations.list.find(c => c.id === id)?.title ??
		`conversation ${id}`;
	const by = (a: Approval) => requestedBy(a, title);
	const conversation = $derived(
		proposal?.conversationId ?? action?.conversationId ?? null,
	);

	function approved(conversation: number) {
		confirming = null;
		setChat(conversation);
		load();
	}

	function closed() {
		confirming = null;
		rejecting = null;
		load();
	}
</script>

<!-- room to read a proposal -->
<Dialog
	title="Approvals"
	resizable
	remember="approvals"
	icon="key"
	{onclose}
	width={780}
	height={Math.min(innerHeight * 0.8, 860)}
>
	<p>Waiting for an admin:</p>
	<ListView
		items={waiting}
		key={approvalKey}
		bind:selected
		columns={[
			{ label: 'No.' },
			{ label: 'Kind' },
			{ label: 'What' },
			{ label: 'Requested by' },
			{ label: 'Requested' },
			{ label: 'Expires' },
		]}
		empty="Nothing waits."
		minHeight={70}
		maxHeight={150}
	>
		{#snippet row(a)}
			<td>{approvalNumber(a)}</td>
			<td>{approvalKind(a)}</td>
			<td class="what">{approvalTitle(a)}</td>
			<td class="what">{by(a)}</td>
			<td>{formatTime(createdAt(a), true)}</td>
			<td>{formatTime(expiresAt(a), true)}</td>
		{/snippet}
	</ListView>
	<p>Decided and expired, last 30 days:</p>
	<ListView
		items={history}
		key={approvalKey}
		bind:selected
		columns={[
			{ label: 'No.' },
			{ label: 'Kind' },
			{ label: 'What' },
			{ label: 'Requested by' },
			{ label: 'Status' },
			{ label: 'Requested' },
		]}
		empty="Nothing yet."
		minHeight={70}
		maxHeight={120}
	>
		{#snippet row(a)}
			<td>{approvalNumber(a)}</td>
			<td>{approvalKind(a)}</td>
			<td class="what">{approvalTitle(a)}</td>
			<td class="what">{by(a)}</td>
			<td>{statusText(a)}</td>
			<td>{formatTime(createdAt(a), true)}</td>
		{/snippet}
	</ListView>
	{#if error}<p class="error">{error}</p>{/if}
	<div class="detail">
		{#if proposal}
			<ProposalDetail {proposal} {view} />
		{:else if action && item}
			<ActionDetail {action} by={by(item)} />
		{:else}
			<p class="hint">
				Proposals of AI agents and deletions (or the agent's changes of
				the remote backup) wait here until an admin decides. A proposal
				approved, the server agent works on it in a new conversation; an
				action confirmed, it runs.
			</p>
		{/if}
		{#if pending && !canConfirm}
			<p class="hint">
				Turn on two-factor login (Start → Two-Factor Login) to confirm
				here, or use the link in the mail.
			</p>
		{/if}
	</div>
	<div class="row-buttons">
		<Button
			onclick={() => (confirming = item ?? null)}
			disabled={!pending || !canConfirm}
		>
			{action ? (action.deletion ? 'Delete…' : 'Confirm…') : 'Approve…'}
		</Button>
		<Button onclick={() => (rejecting = item ?? null)} disabled={!pending}>
			Reject…
		</Button>
		<Button
			onclick={() => (view = view === 'raw' ? 'formatted' : 'raw')}
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
			onclick={() => conversation && setChat(conversation)}
			disabled={!conversation}
		>
			Open Chat
		</Button>
		<span class="gap"></span>
		<Button primary onclick={onclose}>Close</Button>
	</div>
</Dialog>

{#if confirming?.kind === 'proposal'}
	<ApproveDialog
		proposal={confirming.proposal}
		onapproved={approved}
		onclose={() => (confirming = null)}
	/>
{:else if confirming?.kind === 'action'}
	<ConfirmActionDialog action={confirming.action} onclose={closed} />
{/if}
{#if rejecting?.kind === 'proposal'}
	<RejectDialog
		title="Proposal #{rejecting.proposal.id}"
		path="/proposals/{rejecting.proposal.id}/reject"
		to={rejecting.proposal.from}
		onclose={closed}
	/>
{:else if rejecting?.kind === 'action'}
	<RejectDialog
		title={rejecting.action.number}
		path="/actions/{rejecting.action.id}/reject"
		to={rejecting.action.conversationId ? "genco's agent" : null}
		onclose={closed}
	/>
{/if}

<style lang="scss">
	p {
		margin: 8px 0 4px;
	}

	// the selected one takes the free height, the buttons stay below
	.detail {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
		overflow: auto;
	}

	.what {
		max-width: 200px;
		overflow: hidden;
		text-overflow: ellipsis;
	}
</style>
