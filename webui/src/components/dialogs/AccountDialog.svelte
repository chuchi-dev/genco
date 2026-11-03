<script lang="ts">
	import { post, type User } from '@/lib/api';
	import { session } from '@/lib/session.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	let { onclose }: { onclose: () => void } = $props();

	let name = $state(session.user?.name ?? '');
	let email = $state(session.user?.email ?? '');
	let password = $state('');

	async function save() {
		session.user = (await post('/users/account', {
			email,
			name,
			password,
		})) as User;
		onclose();
	}
</script>

<Dialog title="Change Email" icon="key" {onclose}>
	<p class="hint">
		If the email changes, the old address gets a mail about it.
	</p>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="acc-name">Name:</label>
		<input id="acc-name" bind:value={name} />
		<label for="acc-email">Email:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input id="acc-email" type="email" autofocus bind:value={email} />
		<label for="acc-current">Current password:</label>
		<input
			id="acc-current"
			type="password"
			autocomplete="current-password"
			bind:value={password}
		/>
	</Form>
</Dialog>

<style lang="scss">
	.hint {
		margin: 0 0 8px;
	}
</style>
