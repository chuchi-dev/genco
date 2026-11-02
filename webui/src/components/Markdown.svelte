<script lang="ts">
	import { parse, type Inline } from '@/lib/markdown';

	// the agent's text with its formatting, see lib/markdown
	let { text }: { text: string } = $props();

	const blocks = $derived(parse(text));
</script>

{#snippet spans(nodes: Inline[])}
	{#each nodes as n, i (i)}
		{#if n.t === 'text'}{n.text}{:else if n.t === 'code'}<code
				>{n.text}</code
			>{:else if n.t === 'b'}<b>{@render spans(n.children)}</b
			>{:else if n.t === 'i'}<i>{@render spans(n.children)}</i
			>{:else if n.t === 'a' && n.internal}<a href={n.href}
				>{@render spans(n.children)}</a
			>{:else if n.t === 'a'}<a
				href={n.href}
				target="_blank"
				rel="noreferrer noopener">{@render spans(n.children)}</a
			>{/if}
	{/each}
{/snippet}

<div class="md">
	{#each blocks as b, i (i)}
		{#if b.t === 'p'}
			<p>{@render spans(b.inline)}</p>
		{:else if b.t === 'h'}
			<p class="h">{@render spans(b.inline)}</p>
		{:else if b.t === 'code'}
			<pre>{b.text}</pre>
		{:else if b.t === 'list'}
			<svelte:element this={b.ordered ? 'ol' : 'ul'}>
				{#each b.items as item, j (j)}
					<!-- the marker is written out: nested items count on
					their own, also inside a list of the other kind -->
					<li style:margin-left="{item.depth * 16}px">
						<span class="marker" aria-hidden="true"
							>{item.ordered ? `${item.n}.` : '•'}</span
						>{@render spans(item.inline)}
					</li>
				{/each}
			</svelte:element>
		{/if}
	{/each}
</div>

<style lang="scss">
	@use 'styles/win' as *;

	.md {
		white-space: normal;

		p {
			margin: 0 0 6px;
			white-space: pre-wrap;
		}

		.h {
			font-weight: bold;
		}

		ul,
		ol {
			margin: 0 0 6px;
			padding-left: 20px;
		}

		li {
			list-style: none;
			position: relative;
			margin: 1px 0;
		}

		.marker {
			position: absolute;
			right: 100%;
			padding-right: 5px;
			white-space: nowrap;
		}

		code {
			font-family: $mono;
			font-size: 0.92em;
			background: #f0f0f0;
			padding: 0 2px;
		}

		pre {
			font-family: $mono;
			font-size: 0.92em;
			background: #f0f0f0;
			padding: 4px 6px;
			margin: 0 0 6px;
			overflow-x: auto;
			white-space: pre;
		}

		> :last-child {
			margin-bottom: 0;
		}
	}
</style>
