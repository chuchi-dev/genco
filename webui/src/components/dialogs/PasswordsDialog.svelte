<script lang="ts">
	import { formatTime, get, post } from '@/lib/api';
	import { dialogs, confirmDelete } from '@/lib/dialogs.svelte';
	import { tree } from '@/lib/tree.svelte';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import ListView from '../win/ListView.svelte';

	// passwords for the admins: logins the agent set up or that admins
	// keep for a client. A value is fetched only when someone asks, the
	// activity log notes who did.
	let { onclose }: { onclose: () => void } = $props();

	type Secret = {
		id: number;
		name: string;
		groupId: number | null;
		username: string;
		url: string;
		note: string;
		createdBy: string;
		updatedAt: number;
	};

	let secrets = $state<Secret[]>([]);
	let selected = $state<number | null>(null);
	// a link of the agent opens it on one password, also when it is open
	$effect(() => {
		const id = dialogs.password;
		if (id === null) return;
		select(id);
		dialogs.password = null;
	});
	let filter = $state('');
	let error = $state<string | null>(null);
	// the value of the selected one while it is shown
	let shown = $state<string | null>(null);
	let copied = $state(false);
	// the form: null is the list, 0 a new one
	let editing = $state<number | null>(null);
	let form = $state({
		name: '',
		groupId: null as number | null,
		username: '',
		url: '',
		note: '',
		value: '',
	});

	const secret = $derived(secrets.find(s => s.id === selected));
	const q = $derived(filter.trim().toLowerCase());
	const visible = $derived(
		secrets.filter(
			s =>
				!q ||
				[s.name, s.username, s.url, s.note, groupName(s.groupId)]
					.join(' ')
					.toLowerCase()
					.includes(q),
		),
	);

	function groupName(id: number | null): string {
		return id === null ? '' : (tree.group(id)?.name ?? '');
	}

	async function load() {
		try {
			secrets = await get('/secrets');
			error = null;
		} catch (e: any) {
			error = e.message;
		}
	}
	load();

	function select(id: number) {
		selected = id;
		shown = null;
		copied = false;
	}

	// the value of the entry selected when it was asked for; another row
	// selected meanwhile doesn't get it
	async function value(): Promise<string | null> {
		const id = selected;
		const v: string = await get(`/secrets/${id}/value`);
		return selected === id ? v : null;
	}

	async function show() {
		try {
			const v = await value();
			if (v !== null) shown = v;
		} catch (e: any) {
			error = e.message;
		}
	}

	async function copy() {
		try {
			const v = shown ?? (await value());
			if (v === null) return;
			await navigator.clipboard.writeText(v);
			copied = true;
			setTimeout(() => (copied = false), 2000);
		} catch (e: any) {
			error = e.message;
		}
	}

	/** only http(s) is a link, the server checks it too */
	const isLink = (url: string) => /^https?:\/\//i.test(url);

	function edit(s: Secret | null) {
		editing = s?.id ?? 0;
		form = {
			name: s?.name ?? '',
			groupId: s?.groupId ?? null,
			username: s?.username ?? '',
			url: s?.url ?? '',
			note: s?.note ?? '',
			value: '',
		};
	}

	async function save() {
		if (editing) await post(`/secrets/${editing}`, form);
		else selected = await post('/secrets', form);
		editing = null;
		shown = null;
		await load();
	}

	async function remove() {
		const deleted =
			secret &&
			(await confirmDelete({
				title: 'Delete Password',
				text: `Delete the password ${secret.name}? It can't be shown again.`,
				path: '/secrets/' + secret.id,
			}));
		if (!deleted) return;
		selected = null;
		await load();
	}
</script>

<Dialog title="Passwords" resizable icon="key" {onclose} width={640}>
	{#if editing !== null}
		<Form
			onsubmit={save}
			oncancel={() => (editing = null)}
			ok={editing ? 'Save' : 'Add'}
		>
			<label for="pw-name">Name:</label>
			<!-- svelte-ignore a11y_autofocus -->
			<input
				id="pw-name"
				autofocus
				placeholder="e.g. Matomo admin"
				bind:value={form.name}
			/>
			<label for="pw-group">Group:</label>
			<select id="pw-group" bind:value={form.groupId}>
				<option value={null}>(the server)</option>
				{#each tree.groups as g (g.id)}
					<option value={g.id}>{g.name}</option>
				{/each}
			</select>
			<label for="pw-user">User:</label>
			<input id="pw-user" autocomplete="off" bind:value={form.username} />
			<label for="pw-url">URL:</label>
			<input id="pw-url" placeholder="https://…" bind:value={form.url} />
			<label for="pw-value">Password:</label>
			<input
				id="pw-value"
				type="password"
				autocomplete="new-password"
				placeholder={editing ? 'empty keeps the current one' : ''}
				bind:value={form.value}
			/>
			<label for="pw-note">Note:</label>
			<input id="pw-note" bind:value={form.note} />
		</Form>
	{:else}
		<input
			type="search"
			class="filter"
			placeholder="Search…"
			aria-label="Search passwords"
			bind:value={filter}
		/>
		<ListView
			items={visible}
			key={s => s.id}
			bind:selected={() => selected, id => id !== null && select(id)}
			columns={[
				{ label: 'Name' },
				{ label: 'Group' },
				{ label: 'User' },
				{ label: 'URL' },
			]}
			empty={secrets.length ? 'Nothing matches.' : 'No passwords yet.'}
			ondblclick={show}
			fill
			minHeight={160}
		>
			{#snippet row(s)}
				<td>{s.name}</td>
				<td>{groupName(s.groupId)}</td>
				<td class="selectable">{s.username}</td>
				<td>
					{#if isLink(s.url)}<a
							href={s.url}
							target="_blank"
							rel="noreferrer noopener">{s.url}</a
						>{:else}{s.url}{/if}
				</td>
			{/snippet}
		</ListView>

		{#if secret}
			<fieldset>
				<legend>{secret.name}</legend>
				<div class="value">
					<input
						readonly
						class="selectable"
						aria-label="Password"
						type={shown === null ? 'password' : 'text'}
						value={shown ?? '••••••••••••'}
					/>
					{#if shown === null}
						<Button onclick={show}>Show</Button>
					{:else}
						<Button onclick={() => (shown = null)}>Hide</Button>
					{/if}
					<Button onclick={copy}>{copied ? 'Copied' : 'Copy'}</Button>
				</div>
				{#if secret.note}<p class="note">{secret.note}</p>{/if}
				<p class="hint">
					By {secret.createdBy}, changed {formatTime(
						secret.updatedAt,
					)}. Showing or copying it goes into the activity log.
				</p>
			</fieldset>
		{/if}

		{#if error}<p class="error">{error}</p>{/if}
		<div class="row-buttons">
			<Button onclick={() => edit(null)}>New…</Button>
			<Button onclick={() => secret && edit(secret)} disabled={!secret}>
				Edit…
			</Button>
			<Button onclick={remove} disabled={!secret}>Delete</Button>
			<span class="gap"></span>
			<Button primary onclick={onclose}>Close</Button>
		</div>
	{/if}
</Dialog>

<style lang="scss">
	@use 'styles/win' as *;

	.filter {
		width: 100%;
		margin-bottom: 6px;
	}

	// the rows are ListView's
	:global(tbody tr.selected) a {
		color: $white;
	}

	fieldset {
		margin: 10px 0 0;
	}

	.value {
		display: flex;
		gap: 6px;

		input {
			flex: 1;
			min-width: 0;
			font-family: $mono;
		}
	}

	.note {
		margin: 6px 0 0;
		user-select: text;
	}

	.hint {
		margin: 6px 0 0;
	}
</style>
