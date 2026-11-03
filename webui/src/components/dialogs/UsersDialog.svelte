<script lang="ts">
	import { get, post, type User } from '@/lib/api';
	import { message, confirmDelete } from '@/lib/dialogs.svelte';
	import { mail } from '@/lib/mail.svelte';
	import { session } from '@/lib/session.svelte';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import ListView from '../win/ListView.svelte';
	import NewPassword from '../NewPassword.svelte';

	let { onclose }: { onclose: () => void } = $props();

	let users = $state<User[]>([]);
	let selected = $state<number | null>(null);
	/// invite by mail or add with a password
	let adding = $state<'invite' | 'password' | null>(null);
	let email = $state('');
	let name = $state('');
	let newPassword = $state('');
	let newPasswordInput = $state<NewPassword>();
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
		const auth = await message.password(
			'Invite',
			`invite ${email} as an admin`,
		);
		if (auth === null) return;
		await post('/users/invite', { email, name, ...auth });
		adding = null;
		email = name = '';

		await load();
		message.show('Invite Sent', `${sentTo} got a link to set a password.`);
	}

	async function add() {
		newPasswordInput?.check();
		const added = email;
		const auth = await message.password(
			'Add User',
			`add ${email} as an admin`,
		);
		if (auth === null) return;
		await post('/users/add', {
			email,
			name,
			newPassword,
			...auth,
		});
		adding = null;
		email = name = newPassword = '';

		await load();
		message.show('User Added', `${added} can log in now.`);
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
		const auth = await message.password(
			'Send Invite',
			`send ${u.name} a new link`,
		);
		if (auth === null) return;
		try {
			await post(`/users/${u.id}/invite`, auth);
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
			{#if adding === 'invite'}
				The new user is an admin of this server and gets a mail with a
				link to set a password.
			{:else}
				The new user is an admin of this server and can log in with this
				password right away.
			{/if}
		</p>
		<Form
			onsubmit={adding === 'invite' ? invite : add}
			oncancel={() => (adding = null)}
			ok={adding === 'invite' ? 'Invite' : 'Add'}
		>
			<label for="u-name">Name:</label>
			<!-- svelte-ignore a11y_autofocus -->
			<input id="u-name" autofocus bind:value={name} />
			<label for="u-email">Email:</label>
			<input id="u-email" type="email" bind:value={email} />
			{#if adding === 'password'}
				<NewPassword
					id="u-password"
					bind:value={newPassword}
					bind:this={newPasswordInput}
				/>
			{/if}
		</Form>
	{:else}
		<p class="hint">
			Every user is an admin of this server.
			{#if mail.off}Mail is off, so add them with a password.{/if}
		</p>
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
			<Button onclick={() => (adding = 'invite')} disabled={mail.off}>
				Invite…
			</Button>
			<Button onclick={() => (adding = 'password')}>Add…</Button>
			<Button onclick={reinvite} disabled={selected === null || mail.off}>
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
