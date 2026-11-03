<script lang="ts">
	import { post, type Proposal } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	// rejects a proposal, the reason goes to the agent that proposed it
	let { proposal, onclose }: { proposal: Proposal; onclose: () => void } =
		$props();

	const touch = matchMedia('(pointer: coarse)').matches;
	let reason = $state('');

	async function reject() {
		await post(`/proposals/${proposal.id}/reject`, { message: reason });
		onclose();
	}
</script>

<Dialog title="Reject Proposal" icon="agent" {onclose} width={440}>
	<Form onsubmit={reject} oncancel={onclose} ok="Reject">
		<label for="rj-reason">Reason:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<textarea
			id="rj-reason"
			rows="4"
			maxlength="2000"
			autofocus
			placeholder="optional, sent to {proposal.from}"
			spellcheck={!touch}
			bind:value={reason}
		></textarea>
	</Form>
</Dialog>
