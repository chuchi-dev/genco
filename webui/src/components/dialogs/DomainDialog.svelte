<script lang="ts">
	import { post, type Kind } from '@/lib/api';
	import { tree } from '@/lib/tree.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import DomainFields from './DomainFields.svelte';

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
	let kind = $state<Kind>('static');
	let phpVersion = $state<string | null>(null);
	let root = $state('public');
	let command = $state<string | null>(null);
	let publicDir = $state<string | null>(null);

	async function save() {
		oncreated(
			await post('/domains', {
				groupId: group,
				name,
				kind,
				phpVersion,
				root,
				command,
				publicDir,
			}),
		);
	}
</script>

<Dialog title="New Domain" icon="domain" {onclose}>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="d-name">Domain:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			id="d-name"
			autofocus
			placeholder="example.ch"
			bind:value={name}
		/>
		<label for="d-group">Group:</label>
		<select id="d-group" bind:value={group}>
			{#each tree.groups as g (g.id)}
				<option value={g.id}>{g.name}</option>
			{/each}
		</select>
		<DomainFields
			bind:kind
			bind:phpVersion
			bind:root
			bind:command
			bind:publicDir
		/>
	</Form>
</Dialog>
