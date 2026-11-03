<script lang="ts">
	import { post, type User } from '@/lib/api';
	import { message } from '@/lib/dialogs.svelte';
	import { session } from '@/lib/session.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

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
		A new email needs the link mailed to it, then the old address gets a
		mail about the change.
	</p>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="acc-name">Name:</label>
		<input id="acc-name" bind:value={name} />
		<label for="acc-email">Email:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input id="acc-email" type="email" autofocus bind:value={email} />
		<label for="acc-current">Current password:</label>
		<input
			id="acc-current"
			type="password"
			autocomplete="current-password"
			bind:value={password}
		/>
		{#if session.user?.totp}
			<label for="acc-code">Two-factor code:</label>
			<input
				id="acc-code"
				inputmode="numeric"
				autocomplete="one-time-code"
				placeholder="For a new email"
				bind:value={code}
			/>
		{/if}
	</Form>
</Dialog>

<style lang="scss">
	.hint {
		margin: 0 0 8px;
	}
</style>
