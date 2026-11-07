<script lang="ts">
	import { post } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import NoteField from '../NoteField.svelte';

	// rejects a proposal or a pending action, the reason goes to who asked
	// for it: the proposing agent, or genco's agent in its conversation
	let {
		title,
		path,
		to,
		onclose,
	}: {
		/** "Proposal #12", "A3" */
		title: string;
		/** the reject route */
		path: string;
		/** who gets the reason, null for nobody */
		to: string | null;
		onclose: () => void;
	} = $props();

	let reason = $state('');

	async function reject() {
		await post(path, { message: reason });
		onclose();
	}
</script>

<Dialog title="Reject {title}" icon="agent" {onclose} width={440}>
	<Form onsubmit={reject} oncancel={onclose} ok="Reject">
		<NoteField
			id="rj-reason"
			label="Reason:"
			placeholder={to
				? `optional, sent to ${to}`
				: 'optional, for the other admins'}
			autofocus
			bind:value={reason}
		/>
	</Form>
</Dialog>
