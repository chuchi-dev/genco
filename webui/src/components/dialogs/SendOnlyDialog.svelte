<script lang="ts">
	import { post, type MailDomain } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	// a send-only address: an app's own SMTP login, genco makes its
	// password
	let {
		domain,
		oncreated,
		onclose,
	}: {
		domain: MailDomain;
		oncreated: (id: number) => void;
		onclose: () => void;
	} = $props();

	let local = $state('noreply');
	let note = $state('');

	async function save() {
		oncreated(
			await post(`/mail-domains/${domain.id}/send-only`, {
				localPart: local,
				note,
			}),
		);
	}
</script>

<Dialog title="New Send-only Address" icon="mail" {onclose} width={440}>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="so-local">Address:</label>
		<span class="address">
			<!-- svelte-ignore a11y_autofocus -->
			<input id="so-local" autofocus bind:value={local} />
			<span>@{domain.name}</span>
		</span>
		<label for="so-note">For:</label>
		<input
			id="so-note"
			placeholder="the shop's order mails"
			bind:value={note}
		/>
	</Form>
	<p class="hint">
		An app sends as this address with its own login, and only as it. It has
		no mailbox: replies go wherever the domain's mail goes.
	</p>
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

	.hint {
		margin: 8px 0 0;
	}
</style>
