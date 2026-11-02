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
	// svelte-ignore state_referenced_locally
	let group = $state(groupId ?? tree.groups[0]?.id ?? 0);

	async function save() {
		oncreated(await post('/mail-domains', { groupId: group, name }));
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
	</Form>
	<p class="hint">
		Mailboxes and forwards for addresses @this domain. Its page shows the
		DNS records to set.
	</p>
</Dialog>

<style>
	.hint {
		margin: 8px 0 0;
	}
</style>
