<script lang="ts">
	import { post } from '@/lib/api';
	import { tree } from '@/lib/tree.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	let {
		groupId,
		oncreated,
		onclose,
	}: {
		groupId: number | null;
		oncreated: (id: number) => void;
		onclose: () => void;
	} = $props();

	let name = $state('');
	let sendOnly = $state(false);
	// svelte-ignore state_referenced_locally
	let group = $state(groupId ?? tree.groups[0]?.id ?? 0);

	async function save() {
		oncreated(
			await post('/mail-domains', { groupId: group, name, sendOnly }),
		);
	}
</script>

<Dialog title="New Mail Domain" icon="mail" {onclose}>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="md-name">Domain:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			id="md-name"
			autofocus
			placeholder="example.ch"
			bind:value={name}
		/>
		<label for="md-group">Group:</label>
		<select id="md-group" bind:value={group}>
			{#each tree.groups as g (g.id)}
				<option value={g.id}>{g.name}</option>
			{/each}
		</select>
		<span>Mail:</span>
		<div class="modes">
			<label>
				<input type="radio" bind:group={sendOnly} value={false} />
				Receive mail here
			</label>
			<label>
				<input type="radio" bind:group={sendOnly} value={true} />
				Only send from apps here (inbox stays elsewhere)
			</label>
		</div>
	</Form>
	<p class="hint">
		{#if sendOnly}
			Apps send as addresses @this domain with their own login, signed;
			mail to it still goes to its inbox elsewhere. Its page shows the DNS
			records to set.
		{:else}
			Mailboxes and forwards for addresses @this domain. Its page shows
			the DNS records to set.
		{/if}
	</p>
</Dialog>

<style>
	.hint {
		margin: 8px 0 0;
	}

	.modes {
		display: flex;
		flex-direction: column;
		gap: 4px;

		label {
			display: flex;
			gap: 6px;
			align-items: center;
		}
	}
</style>
