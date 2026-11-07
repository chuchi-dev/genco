<script lang="ts">
	import { message } from '@/lib/dialogs.svelte';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Icon from '../win/Icon.svelte';

	// the message, question or input of `message`, see lib/dialogs
	const box = $derived(message.current);
	let value = $state('');
	let code = $state('');

	// a new box starts with its own value and without the code of the
	// last one
	$effect.pre(() => {
		if (!box) return;
		value = box.value;
		code = '';
	});

	function submit(e: SubmitEvent) {
		e.preventDefault();
		message.answer(
			box?.kind === 'ask' || box?.kind === 'password' ? value : '',
			code,
		);
	}
</script>

{#if box}
	<Dialog title={box.title} onclose={() => message.answer(null)} width={420}>
		<form onsubmit={submit}>
			<div class="msg">
				<Icon name="agent" size={32} />
				<div class="text">
					<p class="selectable">{box.text}</p>
					{#if box.kind === 'ask' || (box.kind === 'password' && !box.code)}
						<input
							aria-label={box.text}
							type={box.kind === 'password' ? 'password' : 'text'}
							autocomplete={box.kind === 'password'
								? 'current-password'
								: 'off'}
							bind:value
							{@attach el => {
								// the typing goes here, a given value is selected
								requestAnimationFrame(() => {
									el.focus();
									el.select();
								});
							}}
						/>
					{/if}
					{#if box.code}
						<input
							aria-label="Two-factor code"
							placeholder="Two-factor code"
							inputmode="numeric"
							autocomplete="one-time-code"
							bind:value={code}
							{@attach el => {
								requestAnimationFrame(() => el.focus());
							}}
						/>
					{/if}
				</div>
			</div>
			<div class="row-buttons center">
				{#if box.kind === 'message'}
					<Button type="submit" primary>OK</Button>
				{:else if box.kind === 'confirm'}
					<Button type="submit" primary>Yes</Button>
					<Button onclick={() => message.answer(null)}>No</Button>
				{:else}
					<Button type="submit" primary>OK</Button>
					<Button onclick={() => message.answer(null)}>Cancel</Button>
				{/if}
			</div>
		</form>
	</Dialog>
{/if}

<style>
	.msg {
		display: flex;
		gap: 12px;
		align-items: flex-start;
	}

	.text {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	p {
		margin: 4px 0 0;
		white-space: pre-wrap;
	}
</style>
