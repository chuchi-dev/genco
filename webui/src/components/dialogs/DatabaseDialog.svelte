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
	let engine = $state<'mariadb' | 'postgres'>('mariadb');
	// svelte-ignore state_referenced_locally
	let group = $state(groupId ?? tree.groups[0]?.id ?? 0);

	async function save() {
		oncreated(await post('/databases', { groupId: group, name, engine }));
	}
</script>

<Dialog title="New Database" icon="database" {onclose}>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="db-name">Name:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input id="db-name" autofocus placeholder="shop" bind:value={name} />
		<label for="db-engine">Server:</label>
		<select id="db-engine" bind:value={engine}>
			<option value="mariadb">MariaDB</option>
			<option value="postgres"
				>PostgreSQL (installed with the first one)</option
			>
		</select>
		<label for="db-group">Group:</label>
		<select id="db-group" bind:value={group}>
			{#each tree.groups as g (g.id)}
				<option value={g.id}>{g.name}</option>
			{/each}
		</select>
	</Form>
</Dialog>
