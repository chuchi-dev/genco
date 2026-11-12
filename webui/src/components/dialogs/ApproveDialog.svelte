<script lang="ts">
	import { post, type Plan, type Proposal } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import SecondFactor from '../SecondFactor.svelte';

	// approves the newest plan of a proposal: genco's agent carries it out.
	// The 2FA code (the password without 2FA) confirms it.
	let {
		proposal,
		plan,
		onapproved,
		onclose,
	}: {
		proposal: Proposal;
		/** the plan the admin read, the server refuses an older one */
		plan: Plan;
		/** gets the new conversation */
		onapproved: (conversation: number) => void;
		onclose: () => void;
	} = $props();

	let password = $state('');
	let code = $state('');

	async function approve() {
		onapproved(
			await post(`/proposals/${proposal.id}/approve`, {
				plan: plan.id,
				password,
				code,
			}),
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
		genco's agent carries out plan v{plan.version} as if you asked{plan
			.deletes.length
			? `, it deletes ${plan.deletes.join(', ')} without asking again`
			: ''}. Its answer goes to {proposal.from}.
	</p>
	<Form onsubmit={approve} oncancel={onclose} ok="Approve">
		<SecondFactor bind:password bind:code />
	</Form>
</Dialog>

<style>
	p {
		margin: 0 0 10px;
	}
</style>
