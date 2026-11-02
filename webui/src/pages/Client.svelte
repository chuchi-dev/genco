<script lang="ts" module>
	import { tree } from '@/lib/tree.svelte';

	export function loadProps({ id }: { id: string }) {
		if (!tree.client(Number(id))) throw new Error('This client is gone.');
	}
</script>

<script lang="ts">
	import { del, post } from '@/lib/api';
	import { dialogs, message } from '@/lib/dialogs.svelte';
	import { go } from '@/lib/router.svelte';
	import Button from '@/components/win/Button.svelte';
	import Form from '@/components/win/Form.svelte';

	let { id }: { id: string } = $props();

	// svelte-ignore state_referenced_locally
	const start = tree.client(Number(id))!;
	const client = $derived(tree.client(Number(id)) ?? start);

	let name = $state(start.name);
	let contact = $state(start.contact);
	let notes = $state(start.notes);

	async function save() {
		await post('/clients/' + client.id, { name, contact, notes });
	}

	async function remove() {
		if (
			!(await message.confirm(
				'Delete Client',
				`Delete the client ${client.name}?`,
			))
		)
			return;
		try {
			await del('/clients/' + client.id);
			go('/');
		} catch (e: any) {
			message.show('Not Deleted', e.message);
		}
	}
</script>

<Form onsubmit={save} ok="Save" done="Saved">
	<label for="cv-name">Name:</label>
	<input id="cv-name" bind:value={name} />
	<label for="cv-contact">Contact:</label>
	<input id="cv-contact" bind:value={contact} />
	<label for="cv-notes">Notes:</label>
	<textarea id="cv-notes" rows="3" bind:value={notes}></textarea>
</Form>

<hr class="groove" />

<div class="list-view">
	<table>
		<thead>
			<tr>
				<th>Group</th>
				<th>Domains</th>
			</tr>
		</thead>
		<tbody>
			{#each client.groups as g (g.id)}
				<tr>
					<td><a href="/groups/{g.id}">{g.name}</a></td>
					<td>{g.domains.map(d => d.name).join(', ')}</td>
				</tr>
			{:else}
				<tr><td colspan="2" class="empty">No groups yet.</td></tr>
			{/each}
		</tbody>
	</table>
</div>
<div class="row-buttons">
	<Button onclick={() => (dialogs.open = 'group')}>New Group…</Button>
	{#if !client.groups.length}
		<Button onclick={remove}>Delete Client</Button>
	{/if}
</div>
