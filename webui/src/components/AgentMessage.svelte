<script lang="ts">
	import { formatTime, type Message } from '@/lib/api';
	import Markdown from './Markdown.svelte';
	import AttachmentView from './AttachmentView.svelte';

	// one message of the chat; a tool call with its output, folded
	let {
		m,
		output,
		live = false,
	}: {
		m: Message;
		/** the output of a tool call, missing while it runs */
		output?: Message;
		/** the answer that is being written, without a time */
		live?: boolean;
	} = $props();

	let open = $state(false);

	/** the first line of what the tool got */
	function summary(input: any): string {
		const s = input?.command ?? input?.path ?? JSON.stringify(input ?? {});
		return String(s).split('\n')[0];
	}

	// an AI agent's proposal reaches the agent in a tag it can't close
	// (proposals.rs), here it reads like the agent's own text
	function proposal(text: string): string | null {
		const inner = text.match(/^<proposal>\n([\s\S]*)\n<\/proposal>$/);
		return inner
			? inner[1].replaceAll('‹', '<').replaceAll('›', '>')
			: null;
	}
</script>

{#if m.kind === 'user' || m.kind === 'assistant'}
	<div class="msg {m.kind}">
		<div class="who">
			{m.kind === 'user' ? (m.userName ?? 'terminal') : 'Agent'}
			{#if !live}<span>{formatTime(m.createdAt, true)}</span>{/if}
		</div>
		<div class="text">
			{#if m.kind === 'assistant'}<Markdown
					text={m.text}
				/>{:else if proposal(m.text) !== null}<Markdown
					text={proposal(m.text) ?? ''}
				/>{:else}{m.text}{/if}
		</div>
		{#if m.kind === 'user' && m.files?.length}
			<div class="files">
				{#each m.files as f (f.id)}
					<AttachmentView file={f} />
				{/each}
			</div>
		{/if}
	</div>
{:else if m.kind === 'toolCall'}
	<div
		class="tool"
		class:failed={output?.kind === 'toolOutput' && output.isError}
	>
		<button onclick={() => (open = !open)} aria-expanded={open}>
			<span>{open ? '▾' : '▸'}</span>
			<b>{m.name}</b>
			<code>{summary(m.input)}</code>
			{#if !output}<span class="busy">…</span>{/if}
		</button>
		{#if open}
			{#if m.name === 'shell' || m.name === 'write_file' || m.name === 'edit_file'}
				<pre class="input">{m.input.command ??
						m.input.content ??
						`${m.input.old_text}\n→\n${m.input.new_text}`}</pre>
			{/if}
			{#if output?.kind === 'toolOutput'}
				<pre>{output.output}</pre>
			{/if}
		{/if}
	</div>
{:else if m.kind === 'system'}
	<div class="note">
		<button class="sys" onclick={() => (open = !open)} aria-expanded={open}>
			{open ? '▾' : '▸'} genco to the agent
		</button>
		{#if open}<div>{m.text}</div>{/if}
	</div>
{:else if m.kind === 'error'}
	<div class="note error">{m.text}</div>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.msg {
		margin: 6px 0 10px;

		.who {
			font-weight: bold;

			span {
				font-weight: normal;
				color: $shadow;
				margin-left: 4px;
			}
		}

		.text {
			white-space: pre-wrap;
			overflow-wrap: anywhere;
			margin-top: 2px;
		}

		&.user .who {
			color: $title;
		}

		.files {
			display: flex;
			flex-wrap: wrap;
			align-items: flex-end;
			gap: 4px 10px;
			margin-top: 4px;
		}
	}

	.tool {
		margin: 2px 0;

		button {
			display: flex;
			align-items: baseline;
			gap: 5px;
			width: 100%;
			border: none;
			background: none;
			font: inherit;
			padding: 1px 0;
			text-align: left;
			min-width: 0;
		}

		code {
			overflow: hidden;
			text-overflow: ellipsis;
			white-space: nowrap;
			color: $hint;
		}

		&.failed b {
			color: $error;
		}

		pre {
			margin: 2px 0 6px 14px;
			padding: 4px 6px;
			background: $terminal-bg;
			color: $terminal-fg;
			max-height: 300px;
			overflow: auto;
			white-space: pre-wrap;
			overflow-wrap: anywhere;

			&.input {
				color: #ffff80;
			}
		}
	}

	.busy {
		color: $shadow;
	}

	.note {
		color: $shadow;
		font-style: italic;
		margin: 6px 0;
		white-space: pre-wrap;

		&.error {
			color: $error;
			font-style: normal;
		}
	}

	.sys {
		border: none;
		background: none;
		font: inherit;
		color: inherit;
		padding: 0;
	}
</style>
