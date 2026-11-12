<script lang="ts">
	import type { Proposal } from '@/lib/api';
	import Markdown from './Markdown.svelte';
	import ProposalText from './ProposalText.svelte';

	// a proposal in Start → Approvals: its plan, who decided and what went
	// back to the agent
	let {
		proposal,
		view,
	}: {
		proposal: Proposal;
		view: 'formatted' | 'raw' | 'request';
	} = $props();

	const approved = $derived(
		proposal.plans.find(p => p.id === proposal.planId),
	);
</script>

<ProposalText {proposal} {view} />
{#if proposal.status === 'pending'}
	<p class="hint">
		Waits for an admin to approve it, ask for a change or reject it{proposal.approverEmail
			? `, the mail went to ${proposal.approverEmail}`
			: ''}.
	</p>
{/if}
{#if proposal.decidedBy}
	<p class="hint">
		{proposal.status === 'rejected' ? 'Rejected' : 'Approved'} by
		{proposal.decidedBy}{approved ? ` (plan v${approved.version})` : ''}.
	</p>
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

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 8px 0 4px;
	}

	// the answer, formatted like the plan
	.text {
		@include sunken;
		flex: 1 1 140px;
		min-height: 80px;
		overflow: auto;
		padding: 4px 8px;
		background: #fff;
	}
</style>
