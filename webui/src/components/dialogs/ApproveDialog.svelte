<script lang="ts">
	import { post, type Proposal } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import NoteField from '../NoteField.svelte';

	// approves a proposal, the comment changes the task: genco's agent
	// follows it where it differs from the proposal, and the proposing
	// agent gets it with the answer
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

	let comment = $state('');

	async function approve() {
		onapproved(
			await post(`/proposals/${proposal.id}/approve`, { comment }),
		);
	}
</script>

<Dialog
	title="Approve Proposal #{proposal.id}"
	icon="agent"
	{onclose}
	width={440}
>
	<p>
		The server agent works on this task from {proposal.from} as if an admin asked.
	</p>
	<Form onsubmit={approve} oncancel={onclose} ok="Approve">
		<NoteField
			id="ap-comment"
			label="Comment:"
			placeholder="optional, genco's agent and {proposal.from} see it, e.g. yes, but keep the old DNS for now"
			autofocus
			bind:value={comment}
		/>
	</Form>
</Dialog>

<style>
	p {
		margin: 0 0 10px;
	}
</style>
