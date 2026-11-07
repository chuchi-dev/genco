<script lang="ts">
	import type { Proposal } from '@/lib/api';
	import Markdown from './Markdown.svelte';

	// a proposal as the admin decides on it, with the warnings: in Start →
	// AI Agents and on the page of the mail's link.
	// raw: the text exactly as stored, what the server agent gets;
	// original: what the agent sent, before the rewrite
	let {
		proposal,
		view,
	}: {
		proposal: Proposal;
		view: 'formatted' | 'raw' | 'original';
	} = $props();
</script>

<p><b>{proposal.from}</b> proposes (#{proposal.id}):</p>
{#if proposal.mixedScripts}
	<p class="error">
		Contains letters, digits or punctuation outside the usual European ones
		(e.g. Cyrillic а, ∕ for /) — check names, addresses and commands
		carefully.
	</p>
{/if}
{#if proposal.rewriteMixedScripts}
	<p class="error">
		The rewrite contains letters, digits or punctuation outside the usual
		European ones — compare it with the original.
	</p>
{/if}
{#if proposal.removedChars}
	<p class="hint">
		{proposal.removedChars} invisible character{proposal.removedChars === 1
			? ''
			: 's'} removed.
	</p>
{/if}
{#if !proposal.rewrittenBy && !proposal.text.includes('\n') && proposal.text.length > 400}
	<p class="hint">The agent sent one long paragraph.</p>
{/if}
{#if view === 'original'}
	<p class="hint">The original the agent sent:</p>
{:else if proposal.rewrittenBy}
	<p class="hint">
		Rewritten for readability by {proposal.rewrittenBy}{proposal.rewriteNote
			? ` (${proposal.rewriteNote})`
			: ''}.
	</p>
{:else if proposal.rewriteTried}
	<p class="hint">
		Not rewritten{proposal.rewriteNote ? `: ${proposal.rewriteNote}` : '.'}
	</p>
{/if}
<div class="text selectable">
	{#if view === 'original'}
		<pre class="raw">{proposal.originalText}</pre>
	{:else if view === 'raw'}
		<pre class="raw">{proposal.text}</pre>
	{:else}
		<Markdown text={proposal.text} hrefs />
	{/if}
</div>

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 8px 0 4px;
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
