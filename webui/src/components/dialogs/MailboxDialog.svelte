<script lang="ts">
	import { post, type MailDomain, type Mailbox } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import NewPassword from '../NewPassword.svelte';

	// a new mailbox, or a new password and quota for one
	let {
		domain,
		mailbox = null,
		onclose,
	}: {
		domain: MailDomain;
		mailbox?: Mailbox | null;
		onclose: () => void;
	} = $props();

	let local = $state('');
	let password = $state('');
	let newPassword: NewPassword;
	// svelte-ignore state_referenced_locally
	let quota = $state(String(mailbox?.quotaMb ?? 2048));

	async function save() {
		newPassword.check();
		const quotaMb = Number(quota);
		if (!Number.isInteger(quotaMb) || quotaMb < 10)
			throw new Error('The quota is a number of MB, at least 10');
		if (mailbox) {
			await post(`/mailboxes/${mailbox.id}`, {
				password: password || null,
				quotaMb,
			});
		} else {
			await post(`/mail-domains/${domain.id}/mailboxes`, {
				localPart: local,
				password,
				quotaMb,
			});
		}
		onclose();
	}
</script>

<Dialog
	title={mailbox ? mailbox.address : 'New Mailbox'}
	icon="mail"
	{onclose}
	width={420}
>
	<Form onsubmit={save} oncancel={onclose}>
		{#if !mailbox}
			<label for="mb-local">Address:</label>
			<span class="address">
				<!-- svelte-ignore a11y_autofocus -->
				<input
					id="mb-local"
					autofocus
					placeholder="info"
					bind:value={local}
				/>
				<span>@{domain.name}</span>
			</span>
		{/if}
		<NewPassword
			id="mb-pw"
			autofocus={!!mailbox}
			placeholder={mailbox ? 'Empty keeps the current one' : undefined}
			bind:value={password}
			bind:this={newPassword}
		/>
		<label for="mb-quota">Quota (MB):</label>
		<input id="mb-quota" inputmode="numeric" bind:value={quota} />
	</Form>
</Dialog>

<style lang="scss">
	.address {
		display: flex;
		align-items: center;
		gap: 2px;

		input {
			flex: 1;
			min-width: 0;
		}
	}
</style>
