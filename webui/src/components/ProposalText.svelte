<script lang="ts">
	import type { Proposal } from '@/lib/api';
	import Markdown from './Markdown.svelte';

	// a proposal as the admin decides on it: the planner's newest plan with
	// its warnings and deletions, in Start → Approvals and on the page of
	// the mail's link. raw: the plan exactly as genco's agent gets it;
	// request: the agent's own text, only the planner reads it
	let {
		proposal,
		view,
	}: {
		proposal: Proposal;
		view: 'formatted' | 'raw' | 'request';
	} = $props();

	// an older version to look at, else the newest
	let shown = $state<number | null>(null);
	const newest = $derived(proposal.plans.at(-1) ?? null);
	const plan = $derived(proposal.plans.find(p => p.id === shown) ?? newest);
	const s = (n: number) => (n === 1 ? '' : 's');
</script>

{#if view === 'request'}
	<p>
		<b>{proposal.from}</b> proposed (#{proposal.id}), only the planner reads
		this:
	</p>
	{#if proposal.mixedScripts}
		<p class="error">
			Contains letters, digits or punctuation outside the usual European
			ones (e.g. Cyrillic а, ∕ for /).
		</p>
	{/if}
	{#if proposal.removedChars}
		<p class="hint">
			{proposal.removedChars} invisible character{s(
				proposal.removedChars,
			)}
			removed.
		</p>
	{/if}
	<div class="text selectable">
		<pre class="raw">{proposal.text}</pre>
	</div>
{:else if !plan}
	<p>
		<b>{proposal.from}</b> proposed a task (#{proposal.id}){proposal.status ===
		'planning'
			? ", genco's planner works on a plan."
			: ' before plans existed:'}
	</p>
	{#if proposal.status !== 'planning'}
		<div class="text selectable">
			<pre class="raw">{proposal.text}</pre>
		</div>
	{/if}
{:else}
	<p>
		<b>{proposal.from}</b> proposed a task (#{proposal.id}), genco's planner {plan.refused
			? 'wrote no plan'
			: 'wrote this plan'}{plan.version > 1
			? ` (version ${plan.version})`
			: ''}:
		{#if proposal.plans.length > 1}
			<select
				aria-label="Version"
				value={plan.id}
				onchange={e => (shown = +e.currentTarget.value)}
			>
				{#each proposal.plans as p (p.id)}
					<option value={p.id}>
						v{p.version}{p === newest ? ' (newest)' : ''}
					</option>
				{/each}
			</select>
		{/if}
	</p>
	{#if plan.note}
		<p class="hint">After {plan.noteBy}'s change: {plan.note}</p>
	{/if}
	{#if plan !== newest}
		<p class="hint">An older version, decisions go to the newest.</p>
	{/if}
	{#if plan.mixedScripts}
		<p class="error">
			Contains letters, digits or punctuation outside the usual European
			ones (e.g. Cyrillic а, ∕ for /) — check names, addresses and
			commands carefully.
		</p>
	{/if}
	{#if plan.removedChars}
		<p class="hint">
			{plan.removedChars} invisible character{s(plan.removedChars)} removed.
		</p>
	{/if}
	{#if plan.deletes.length}
		<p class="error">
			Deletes without asking again: {plan.deletes.join(', ')}
		</p>
	{/if}
	{#if plan.otherDeletes.length}
		<p class="hint">
			{plan.otherDeletes.length} Delete line{s(plan.otherDeletes.length)}
			don't name one domain, database, group, mail domain or mailbox: those
			deletions ask again.
		</p>
	{/if}
	<div class="text selectable">
		{#if view === 'raw' || plan.refused}
			<pre class="raw">{plan.text}</pre>
		{:else}
			<Markdown text={plan.text} hrefs />
		{/if}
	</div>
{/if}
{#if proposal.status === 'planning' && proposal.changeNote}
	<p class="hint">
		The planner works on {proposal.changeBy}'s change: {proposal.changeNote}
	</p>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 8px 0 4px;
	}

	select {
		margin-left: 4px;
	}

	// formatted like the chat, takes the height of a bigger window
	.text {
		@include sunken;
		flex: 1 1 140px;
		min-height: 80px;
		overflow: auto;
		padding: 4px 8px;
		background: #fff;
	}

	.raw {
		margin: 0;
		font-family: $mono;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
</style>
