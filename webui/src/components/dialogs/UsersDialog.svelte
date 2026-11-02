<script lang="ts">
	import { get, post, type User } from '@/lib/api';
	import { message, confirmDelete } from '@/lib/dialogs.svelte';
	import { session } from '@/lib/session.svelte';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import ListView from '../win/ListView.svelte';

	let { onclose }: { onclose: () => void } = $props();

	let users = $state<User[]>([]);
	let selected = $state<number | null>(null);
	let adding = $state(false);
	let email = $state('');
	let name = $state('');
	let error = $state<string | null>(null);

	const canRemove = $derived(
		selected !== null && selected !== session.user?.id,
	);

	async function load() {
		try {
			users = await get('/users');
			error = null;
		} catch (e: any) {
			error = e.message;
		}
	}
	load();

	async function invite() {
		const sentTo = email;
		const password = await message.password(
			'Invite',
			`invite ${email} as an admin`,
		);
		if (password === null) return;
		await post('/users/invite', { email, name, password });
		adding = false;
		email = name = '';

		await load();
		message.show('Invite Sent', `${sentTo} got a link to set a password.`);
	}

	/// a new link, also when someone forgot the password
	async function reinvite() {
		const u = users.find(u => u.id === selected);
		if (
			!u ||
			!(await message.confirm(
				'Send Invite',
				`Send ${u.name} a new link to set a password? The current password keeps working until then.`,
			))
		)
			return;
		const password = await message.password(
			'Send Invite',
			`send ${u.name} a new link`,
		);
		if (password === null) return;
		try {
			await post(`/users/${u.id}/invite`, { password });
			message.show('Invite Sent', `${u.email} got a new link.`);
		} catch (e: any) {
			error = e.message;
		}
	}

	async function remove() {
		const u = users.find(u => u.id === selected);
		const removed =
			u &&
			(await confirmDelete({
				title: 'Remove User',
				text: `Remove ${u.name} (${u.email})?`,
				path: '/users/' + u.id,
				password: `remove ${u.name}`,
			}));
		if (!removed) return;
		selected = null;
		await load();
	}
</script>

<Dialog title="Users" resizable icon="users" {onclose} width={460}>
	{#if adding}
		<p class="hint">
			The new user is an admin of this server and gets a mail with a link
			to set a password.
		</p>
		<Form onsubmit={invite} oncancel={() => (adding = false)} ok="Invite">
			<label for="u-name">Name:</label>
			<!-- svelte-ignore a11y_autofocus -->
			<input id="u-name" autofocus bind:value={name} />
			<label for="u-email">Email:</label>
			<input id="u-email" type="email" bind:value={email} />
		</Form>
	{:else}
		<p class="hint">Every user is an admin of this server.</p>
		<ListView
			items={users}
			key={u => u.id}
			bind:selected
			columns={[{ label: 'Name' }, { label: 'Email' }]}
			fill
			minHeight={140}
		>
			{#snippet row(u)}
				<td>{u.name}{u.id === session.user?.id ? ' (you)' : ''}</td>
				<td>{u.email}{u.invited ? ' (invited)' : ''}</td>
			{/snippet}
		</ListView>
		{#if error}<p class="error">{error}</p>{/if}
		<div class="row-buttons">
			<Button onclick={() => (adding = true)}>Invite…</Button>
			<Button onclick={reinvite} disabled={selected === null}>
				Send Link
			</Button>
			<Button onclick={remove} disabled={!canRemove}>Remove</Button>
			<span class="gap"></span>
			<Button primary onclick={onclose}>Close</Button>
		</div>
	{/if}
</Dialog>

<style lang="scss">
	@use 'styles/win' as *;
	.hint {
		margin: 0 0 8px;
	}

	.error {
		margin: 8px 0 0;
	}
</style>
