<script lang="ts">
	import { post, type Proposal } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	// approves a proposal, the comment changes the task: genco's agent
	// follows it where it differs from the proposal
	let {
		proposal,
		onapproved,
		onclose,
	}: {
		proposal: Proposal;
		/** gets the new conversation */
		onapproved: (conversation: number) => void;
		onclose: () => void;
	} = $props();

	const touch = matchMedia('(pointer: coarse)').matches;
	let comment = $state('');

	async function approve() {
		onapproved(
			await post(`/proposals/${proposal.id}/approve`, { comment }),
		);
	}
</script>

<Dialog title="Approve Proposal" icon="agent" {onclose} width={440}>
	<p>
		The server agent works on this task from {proposal.from} as if an admin asked.
	</p>
	<Form onsubmit={approve} oncancel={onclose} ok="Approve">
		<label for="ap-comment">Comment for genco's agent:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<textarea
			id="ap-comment"
			rows="4"
			maxlength="2000"
			autofocus
			placeholder="optional, e.g. yes, but keep the old DNS for now"
			spellcheck={!touch}
			bind:value={comment}
		></textarea>
	</Form>
</Dialog>

<style>
	p {
		margin: 0 0 10px;
	}
</style>
