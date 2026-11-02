<script lang="ts">
	import { post } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	let { onclose }: { onclose: () => void } = $props();

	let current = $state('');
	let password = $state('');
	let repeat = $state('');

	async function save() {
		if (password !== repeat) throw new Error("The passwords don't match.");
		await post('/users/password', { current, password });
		onclose();
	}
</script>

<Dialog title="Change Password" icon="key" {onclose}>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="pw-current">Current password:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			id="pw-current"
			type="password"
			autocomplete="current-password"
			autofocus
			bind:value={current}
		/>
		<label for="pw-new">New password:</label>
		<input
			id="pw-new"
			type="password"
			autocomplete="new-password"
			bind:value={password}
		/>
		<label for="pw-repeat">Confirm:</label>
		<input
			id="pw-repeat"
			type="password"
			autocomplete="new-password"
			bind:value={repeat}
		/>
	</Form>
</Dialog>
