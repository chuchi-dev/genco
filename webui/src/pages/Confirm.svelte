<script lang="ts" module>
	import { get } from '@/lib/api';

	export async function loadProps({ token }: { token: string }) {
		return { action: await get(`/actions/${token}`) };
	}
</script>

<script lang="ts">
	import { post, type PendingAction } from '@/lib/api';
	import { go } from '@/lib/router.svelte';
	import { session } from '@/lib/session.svelte';
	import Button from '@/components/win/Button.svelte';
	import Dialog from '@/components/win/Dialog.svelte';
	import Form from '@/components/win/Form.svelte';
	import Icon from '@/components/win/Icon.svelte';

	// the link of a confirmation mail: an admin confirms a deletion or the
	// agent's change of the remote backup; with mail off nothing was
	// mailed, the password and code confirm it
	let {
		token,
		action,
	}: {
		token: string;
		action: PendingAction;
	} = $props();

	// the link's token doesn't stay in the address bar and the history
	history.replaceState(history.state, '', '/confirm');

	let busy = $state(false);
	let result = $state<{ ok: boolean; log: string[] } | null>(null);
	let error = $state<string | null>(null);
	let password = $state('');
	let code = $state('');
	const verb = $derived(action.deletion ? 'Delete' : 'Confirm');

	async function confirmIt() {
		busy = true;
		try {
			result = await post(`/actions/${token}/confirm`, {});
		} catch (e: any) {
			error = e.message;
		}
		busy = false;
	}

	async function confirmWithPassword() {
		result = await post(`/actions/${token}/confirm`, { password, code });
	}
</script>

<Dialog
	title="Confirm {action.number}"
	resizable
	icon="key"
	onclose={() => go('/')}
	keep={() => busy}
	width={520}
>
	<div class="msg">
		<Icon name="agent" size={32} />
		<div>
			<p>
				<b>{action.requestedBy}</b>
				asked for this:
			</p>
			<p class="selectable">{action.description}</p>
		</div>
	</div>
	{#if result}
		<p>{result.ok ? 'Done.' : 'It failed:'}</p>
		<pre class="log selectable">{result.log.join('\n')}</pre>
		<div class="row-buttons end">
			<Button primary onclick={() => go('/')}>Close</Button>
		</div>
	{:else if action.status !== 'pending'}
		<p>This request is {action.status}, nothing to confirm.</p>
		<div class="row-buttons end">
			<Button primary onclick={() => go('/')}>Close</Button>
		</div>
	{:else if action.byPassword}
		<Form onsubmit={confirmWithPassword} oncancel={() => go('/')} ok={verb}>
			<label for="c-password">Your password:</label>
			<!-- svelte-ignore a11y_autofocus -->
			<input
				id="c-password"
				type="password"
				autocomplete="current-password"
				autofocus
				bind:value={password}
			/>
			{#if session.user?.totp}
				<label for="c-code">Two-factor code:</label>
				<input
					id="c-code"
					inputmode="numeric"
					autocomplete="one-time-code"
					bind:value={code}
				/>
			{/if}
		</Form>
	{:else}
		{#if error}<p class="error">{error}</p>{/if}
		<div class="row-buttons end">
			<Button onclick={confirmIt} disabled={busy}>
				{busy ? 'Working…' : verb}
			</Button>
			<Button primary onclick={() => go('/')}>Cancel</Button>
		</div>
	{/if}
</Dialog>

<style lang="scss">
	@use 'styles/win' as *;

	.msg {
		display: flex;
		gap: 12px;
	}

	p {
		margin: 0 0 8px;
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
