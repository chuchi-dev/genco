<script lang="ts">
	import type { Chat } from '@/lib/chat.svelte';
	import Button from './win/Button.svelte';

	// where a message to the agent is written, with Stop while it works
	// and Resolved when a routine or alert needs attention
	let { chat }: { chat: Chat } = $props();

	const touch = matchMedia('(pointer: coarse)').matches;

	let text = $state('');
	let sending = $state(false);

	async function send() {
		const msg = text.trim();
		if (!msg || sending) return;
		// what is typed while it sends stays, a failed message comes back
		sending = true;
		text = '';
		try {
			await chat.send(msg);
		} catch {
			text = text ? `${msg}\n${text}` : msg;
		}
		sending = false;
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey && !touch) {
			e.preventDefault();
			send();
		}
	}
</script>

<div class="composer">
	<textarea
		rows="3"
		placeholder="Message the agent…"
		spellcheck={!touch}
		bind:value={text}
		{onkeydown}
	></textarea>
	<div class="buttons">
		<Button primary onclick={send} disabled={!text.trim() || sending}>
			Send
		</Button>
		{#if chat.running}
			<Button onclick={() => chat.stop()}>Stop</Button>
		{/if}
		{#if chat.conversation?.report === 'attention' && !chat.running}
			<Button
				title="An admin took care of it"
				onclick={() => chat.resolve()}
			>
				Resolved
			</Button>
		{/if}
	</div>
</div>

<style lang="scss">
	.composer {
		display: flex;
		gap: 4px;
		margin-top: 4px;

		textarea {
			flex: 1;
			resize: none;
		}

		.buttons {
			display: flex;
			flex-direction: column;
			gap: 4px;
		}
	}
</style>
