<script lang="ts">
	import { post, type User } from '@/lib/api';
	import { message } from '@/lib/dialogs.svelte';
	import { mail } from '@/lib/mail.svelte';
	import { session } from '@/lib/session.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import SecondFactor from '../SecondFactor.svelte';

	let { onclose }: { onclose: () => void } = $props();

	let name = $state(session.user?.name ?? '');
	let email = $state(session.user?.email ?? '');
	let password = $state('');
	let code = $state('');

	async function save() {
		const resp = (await post('/users/account', {
			email,
			name,
			password,
			code,
		})) as { user: User; confirmSent: string | null };
		session.user = resp.user;
		onclose();
		if (resp.confirmSent)
			message.show(
				'Confirm Email',
				`We sent a link to ${resp.confirmSent}. Open it to use the new email, until then you log in with ${resp.user.email}.`,
			);
	}
</script>

<Dialog title="Change Email" icon="key" {onclose}>
	<p class="hint">
		{#if mail.off}
			Mail is off, a new email counts right away.
		{:else}
			A new email needs the link mailed to it, then the old address gets a
			mail about the change.
		{/if}
	</p>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="acc-name">Name:</label>
		<input id="acc-name" bind:value={name} />
		<label for="acc-email">Email:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input id="acc-email" type="email" autofocus bind:value={email} />
		<SecondFactor bind:password bind:code />
	</Form>
</Dialog>

<style lang="scss">
	.hint {
		margin: 0 0 8px;
	}
</style>
