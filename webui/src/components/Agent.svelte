<script lang="ts">
	import { untrack } from 'svelte';
	import { Chat } from '@/lib/chat.svelte';
	import { conversations } from '@/lib/conversations.svelte';
	import { listen } from '@/lib/events.svelte';
	import { Follow } from '@/lib/follow.svelte';
	import { current as page } from '@/lib/router.svelte';
	import Button from './win/Button.svelte';
	import DropMenu from './win/DropMenu.svelte';
	import Window from './win/Window.svelte';
	import AgentMessage from './AgentMessage.svelte';
	import Composer from './Composer.svelte';
	import ConversationPicker from './ConversationPicker.svelte';

	let { onminimize }: { onminimize: () => void } = $props();

	// follows new text, unless the reader scrolled up
	const follow = new Follow();
	const chat = new Chat(page.chat, () => follow.down());
	chat.load();
	const conversation = $derived(chat.conversation);
	const running = $derived(chat.running);
	// the conversation a new one replaced after a break
	const resume = $derived(
		conversations.list.find(c => c.id === chat.resume) ?? null,
	);

	// another conversation opened elsewhere: the tray, a link of a mail
	$effect(() => {
		const id = page.chat;
		untrack(() => {
			if (id !== chat.current) chat.select(id);
		});
	});

	$effect(() => listen(ev => chat.onEvent(ev)));
</script>

<Window title="Server Agent" icon="agent" {onminimize}>
	<div class="bar">
		<ConversationPicker
			conversations={conversations.list}
			current={chat.conversation}
			onselect={id => chat.select(id)}
		/>
		<Button onclick={() => chat.select(null)}>New</Button>
		<DropMenu
			align="right"
			entries={chat.routines.map(r => ({
				label: r.title + ' now',
				onclick: () => chat.runRoutine(r),
			}))}
		>
			{#snippet button({ toggle })}
				<Button onclick={toggle}>Run ▾</Button>
			{/snippet}
		</DropMenu>
	</div>

	<div class="log-box">
		<div
			class="log selectable"
			class:loading={chat.loading}
			{@attach follow.attach}
		>
			{#if chat.loading}
				<p class="hint">Loading conversation…</p>
			{:else if chat.current === null}
				{#if resume}
					<p class="resume">
						<button
							class="link"
							onclick={() => chat.select(resume.id)}
							>Continue {resume.title}</button
						>
					</p>
				{/if}
				<p class="hint">
					Ask the agent anything about this server: set up a domain,
					check why a site is down, update packages. It has root
					access.
				</p>
			{/if}
			{#each chat.messages as m (m.id)}
				{#if m.kind !== 'toolOutput'}
					<AgentMessage
						{m}
						output={m.kind === 'toolCall'
							? chat.outputs.get(m.callId)
							: undefined}
					/>
				{/if}
			{/each}
			{#if chat.streaming}
				<AgentMessage
					m={{
						id: 0,
						conversationId: chat.current ?? 0,
						createdAt: Date.now(),
						kind: 'assistant',
						text: chat.streaming,
					}}
					live
				/>
			{/if}
			{#if chat.thinking}
				<p class="hint thinking">Thinking<span class="dots"></span></p>
			{/if}
		</div>
		{#if follow.unseen}
			<div class="unseen">
				<Button onclick={() => follow.down()}>↓ New messages</Button>
			</div>
		{/if}
	</div>

	{#if chat.error}<p class="send-error">{chat.error}</p>{/if}
	<Composer {chat} />

	{#snippet status()}
		<div class="status">
			{running ? 'Working…' : conversation ? 'Ready' : 'New conversation'}
		</div>
		{#if conversation?.report}
			<div
				class="status"
				class:attention={conversation.report === 'attention'}
			>
				{conversation.report === 'attention' ? 'Needs attention' : 'OK'}
			</div>
		{/if}
	{/snippet}
</Window>

<style lang="scss">
	@use 'styles/win' as *;

	.bar {
		display: flex;
		gap: 4px;
		margin-bottom: 4px;

		:global(button) {
			min-width: 50px;
		}
	}

	.log-box {
		position: relative;
		flex: 1;
		display: flex;
		flex-direction: column;
		min-height: 0;
	}

	.log {
		@include sunken;
		flex: 1;
		overflow: auto;
		padding: 6px 8px;
		min-height: 0;

		&.loading {
			cursor: wait;
		}
	}

	.unseen {
		position: absolute;
		right: 20px;
		bottom: 6px;
	}

	.hint {
		color: $shadow;
		margin: 4px 0;
	}

	.resume {
		margin: 2px 0 8px;
	}

	.thinking .dots::after {
		content: '';
		animation: dots 1.2s steps(4) infinite;
	}

	@keyframes dots {
		25% {
			content: '.';
		}
		50% {
			content: '..';
		}
		75% {
			content: '...';
		}
	}

	.send-error {
		color: $error;
		margin: 4px 0 0;
	}

	.status {
		@include groove;
		flex: 1;
		padding: 2px 4px;

		& + .status {
			flex: 0 0 auto;
		}

		&.attention {
			color: $error;
		}
	}
</style>
