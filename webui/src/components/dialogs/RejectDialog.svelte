<script lang="ts">
	import { post, type Proposal } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import NoteField from '../NoteField.svelte';

	// rejects a proposal, the reason goes to the agent that proposed it
	let { proposal, onclose }: { proposal: Proposal; onclose: () => void } =
		$props();

	let reason = $state('');

	async function reject() {
		await post(`/proposals/${proposal.id}/reject`, { message: reason });
		onclose();
	}
</script>

<Dialog
	title="Reject Proposal #{proposal.id}"
	icon="agent"
	{onclose}
	width={440}
>
	<Form onsubmit={reject} oncancel={onclose} ok="Reject">
		<NoteField
			id="rj-reason"
			label="Reason:"
			placeholder="optional, sent to {proposal.from}"
			autofocus
			bind:value={reason}
		/>
	</Form>
</Dialog>
