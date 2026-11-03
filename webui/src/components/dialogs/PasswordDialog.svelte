<script lang="ts">
	import { post } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import NewPassword from '../NewPassword.svelte';

	let { onclose }: { onclose: () => void } = $props();

	let current = $state('');
	let password = $state('');
	let newPassword: NewPassword;

	async function save() {
		newPassword.check();
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
		<NewPassword
			id="pw-new"
			label="New password:"
			bind:value={password}
			bind:this={newPassword}
		/>
	</Form>
</Dialog>
