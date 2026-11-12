<script lang="ts">
	import { post, type Plan, type Proposal } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import NoteField from '../NoteField.svelte';

	// sends the newest plan back to the planner with the admin's change; the
	// new version needs an approval again, so no second factor
	let {
		proposal,
		plan,
		onclose,
	}: {
		proposal: Proposal;
		/** the plan the admin read, the server refuses an older one */
		plan: Plan;
		onclose: () => void;
	} = $props();

	let note = $state('');

	async function change() {
		await post(`/proposals/${proposal.id}/change`, {
			plan: plan.id,
			note,
		});
		onclose();
	}
</script>

<Dialog
	title="Change Plan of Proposal #{proposal.id}"
	icon="agent"
	{onclose}
	width={440}
>
	<p>The planner writes a new version, you decide on it again.</p>
	<Form onsubmit={change} oncancel={onclose} ok="Send">
		<NoteField
			id="cp-note"
			label="Change:"
			placeholder="e.g. keep the old DNS for now, only the shop"
			autofocus
			bind:value={note}
		/>
	</Form>
</Dialog>

<style>
	p {
		margin: 0 0 10px;
	}
</style>
