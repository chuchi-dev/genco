<script lang="ts">
	import { untrack } from 'svelte';
	import { Chat } from '@/lib/chat.svelte';
	import { conversations } from '@/lib/conversations.svelte';
	import { listen } from '@/lib/events.svelte';
	import { current as page } from '@/lib/router.svelte';
	import Button from './win/Button.svelte';
	import DropMenu from './win/DropMenu.svelte';
	import Window from './win/Window.svelte';
	import AgentMessage from './AgentMessage.svelte';
	import Composer from './Composer.svelte';
	import ConversationPicker from './ConversationPicker.svelte';

	let { onminimize }: { onminimize: () => void } = $props();

	let scroller: HTMLElement | undefined = $state();

	const chat = new Chat(page.chat, scrollDown);
	chat.load();
	const conversation = $derived(chat.conversation);
	const running = $derived(chat.running);

	// another conversation opened elsewhere: the tray, a link of a mail
	$effect(() => {
		const id = page.chat;
		untrack(() => {
			if (id !== chat.current) chat.select(id);
		});
	});

	$effect(() => listen(ev => chat.onEvent(ev)));

	/// follows new text, unless the reader scrolled up
	function scrollDown(always = false) {
		if (!scroller) return;
		const s = scroller;
		const atEnd = s.scrollHeight - s.scrollTop - s.clientHeight < 60;
		if (!always && !atEnd) return;
		requestAnimationFrame(() => (s.scrollTop = s.scrollHeight));
	}
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

	<div class="log selectable" bind:this={scroller}>
		{#if chat.current === null}
			<p class="hint">
				Ask the agent anything about this server: set up a domain, check
				why a site is down, update packages. It has root access.
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

	.log {
		@include sunken;
		flex: 1;
		overflow: auto;
		padding: 6px 8px;
		min-height: 0;
	}

	.hint {
		color: $shadow;
		margin: 4px 0;
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
