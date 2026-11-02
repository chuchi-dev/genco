<script lang="ts">
	import { post, type MailDomain, type Mailbox } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

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
	let repeat = $state('');
	// svelte-ignore state_referenced_locally
	let quota = $state(String(mailbox?.quotaMb ?? 2048));

	async function save() {
		if (password !== repeat) throw new Error("The passwords don't match");
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
		<label for="mb-pw">Password:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			id="mb-pw"
			type="password"
			autocomplete="new-password"
			autofocus={!!mailbox}
			placeholder={mailbox
				? 'empty keeps the current one'
				: '10 characters or more'}
			bind:value={password}
		/>
		<label for="mb-repeat">Repeat:</label>
		<input
			id="mb-repeat"
			type="password"
			autocomplete="new-password"
			bind:value={repeat}
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
