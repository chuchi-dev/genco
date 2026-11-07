<script lang="ts">
	import { post, type PendingAction } from '@/lib/api';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import SecondFactor from '../SecondFactor.svelte';

	// confirms a pending action in Approvals with the 2FA code (the
	// password without 2FA), then shows what it did
	let { action, onclose }: { action: PendingAction; onclose: () => void } =
		$props();

	let password = $state('');
	let code = $state('');
	let result = $state<{ ok: boolean; log: string[] } | null>(null);
	const verb = $derived(action.deletion ? 'Delete' : 'Confirm');

	async function confirmIt() {
		result = await post(`/actions/${action.id}/confirm`, {
			password,
			code,
		});
		return false;
	}
</script>

<Dialog
	title="{verb} {action.number}"
	icon="key"
	resizable={result !== null}
	{onclose}
	width={480}
>
	<p class="selectable">{action.description}</p>
	{#if result}
		<p>{result.ok ? 'Done.' : 'It failed:'}</p>
		<pre class="log selectable">{result.log.join('\n')}</pre>
		<div class="row-buttons end">
			<Button primary onclick={onclose}>Close</Button>
		</div>
	{:else}
		<Form onsubmit={confirmIt} oncancel={onclose} ok={verb}>
			<SecondFactor autofocus bind:password bind:code />
		</Form>
	{/if}
</Dialog>

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 0 0 10px;
	}

	.log {
		@include sunken;
		margin: 0;
		padding: 6px;
		flex: 1;
		min-height: 120px;
		overflow: auto;
		white-space: pre-wrap;
	}
</style>
