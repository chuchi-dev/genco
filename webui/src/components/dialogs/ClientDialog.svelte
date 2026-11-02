<script lang="ts">
	import { post } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	let {
		oncreated,
		onclose,
	}: { oncreated: (id: number) => void; onclose: () => void } = $props();

	let name = $state('');
	let contact = $state('');

	async function save() {
		oncreated(await post('/clients', { name, contact }));
	}
</script>

<Dialog title="New Client" icon="client" {onclose}>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="c-name">Name:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input id="c-name" autofocus bind:value={name} />
		<label for="c-contact">Contact:</label>
		<input id="c-contact" placeholder="email, phone" bind:value={contact} />
	</Form>
</Dialog>
