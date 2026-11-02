<script lang="ts">
	import { post, type MailDomain } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	// an address that only forwards; `*` takes every address the domain
	// has no mailbox or forward for
	let { domain, onclose }: { domain: MailDomain; onclose: () => void } =
		$props();

	let local = $state('');
	let targets = $state('');

	async function save() {
		await post(`/mail-domains/${domain.id}/forwards`, {
			localPart: local,
			targets: targets.split(/[\s,;]+/).filter(Boolean),
		});
		onclose();
	}
</script>

<Dialog title="New Forward" icon="mail" {onclose} width={440}>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="fw-local">Address:</label>
		<span class="address">
			<!-- svelte-ignore a11y_autofocus -->
			<input
				id="fw-local"
				autofocus
				placeholder="sales or *"
				bind:value={local}
			/>
			<span>@{domain.name}</span>
		</span>
		<label for="fw-to">Forward to:</label>
		<textarea
			id="fw-to"
			rows="3"
			placeholder="one address per line"
			bind:value={targets}
		></textarea>
	</Form>
	<p class="hint">
		<code>*</code> is the catch-all: mail to any address without its own mailbox
		or forward.
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
