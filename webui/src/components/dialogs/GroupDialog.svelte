<script lang="ts">
	import { post } from '@/lib/api';
	import { tree } from '@/lib/tree.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	let {
		clientId,
		oncreated,
		onclose,
	}: {
		clientId: number | null;
		oncreated: (id: number) => void;
		onclose: () => void;
	} = $props();

	let name = $state('');
	// svelte-ignore state_referenced_locally
	let client = $state(clientId ?? tree.clients[0]?.id ?? 0);

	async function save() {
		oncreated(await post('/groups', { name, clientId: client }));
	}
</script>

<Dialog title="New Group" icon="group" {onclose}>
	<p class="hint">
		A group is one project: its domains share files, a Linux user and the
		databases. The name can't be changed later.
	</p>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="g-name">Name:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			id="g-name"
			autofocus
			placeholder="e.g. shop"
			bind:value={name}
		/>
		<label for="g-client">Client:</label>
		<select id="g-client" bind:value={client}>
			{#each tree.clients as c (c.id)}
				<option value={c.id}>{c.name}</option>
			{/each}
		</select>
	</Form>
</Dialog>

<style>
	.hint {
		margin: 0 0 12px;
	}
</style>
