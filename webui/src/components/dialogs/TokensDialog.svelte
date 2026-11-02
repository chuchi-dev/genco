<script lang="ts">
	import OneTimeSecret from '../OneTimeSecret.svelte';
	import { formatTime, get, post, type Token } from '@/lib/api';
	import { message, confirmDelete } from '@/lib/dialogs.svelte';
	import { tree } from '@/lib/tree.svelte';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';
	import ListView from '../win/ListView.svelte';

	// every token of the server, a new one belongs to the logged in admin
	let { onclose }: { onclose: () => void } = $props();

	let tokens = $state<Token[]>([]);
	let selected = $state<number | null>(null);
	let adding = $state(false);
	let name = $state('');
	let scope = $state<'databases' | 'read'>('read');
	let created = $state<string | null>(null);
	// the scope of the token just created, for its example
	let createdScope = $state<'databases' | 'read'>('read');
	let error = $state<string | null>(null);

	async function load() {
		try {
			tokens = await get('/tokens');
			error = null;
		} catch (e: any) {
			error = e.message;
		}
	}
	load();

	function scopeText(scope: string): string {
		if (scope === 'databases') return 'download databases';
		if (scope === 'read') return 'read status and usage';
		const [kind, id] = scope.split(':');
		if (kind === 'deploy')
			return `deploy ${tree.group(Number(id))?.name ?? '#' + id}`;
		return scope;
	}

	async function add() {
		const password = await message.password(
			'New Token',
			'create an API token',
		);
		if (password === null) return;
		created = await post('/tokens', { name, scopes: [scope], password });
		createdScope = scope;
		adding = false;
		name = '';
		await load();
	}

	async function remove() {
		const t = tokens.find(t => t.id === selected);
		const deleted =
			t &&
			(await confirmDelete({
				title: 'Delete Token',
				text: `Delete the token ${t.name}? What uses it stops working.`,
				path: '/tokens/' + t.id,
			}));
		if (!deleted) return;
		selected = null;
		await load();
	}

	const auth = $derived(
		`curl -fsS -H "Authorization: Bearer ${created ?? '<token>'}" \\\n `,
	);
	const examples = $derived([
		{
			scope: 'read',
			text: 'Read the status (services, deploys, backups, problems):',
			code: `${auth} ${location.origin}/api/status`,
		},
		{
			scope: 'databases',
			text: 'Download a database:',
			code: `${auth} ${location.origin}/api/databases/<name>/dump -o db.sql.gz`,
		},
	]);
</script>

<Dialog title="API Tokens" resizable icon="key" {onclose} width={560}>
	{#if adding}
		<p>A token belongs to you and goes when your account goes.</p>
		<Form onsubmit={add} oncancel={() => (adding = false)} ok="Create">
			<span class="label">May:</span>
			<div class="scopes">
				<label>
					<input type="radio" value="read" bind:group={scope} />
					read the status and usage of the server, change nothing (e.g.
					a dashboard)
				</label>
				<label>
					<input type="radio" value="databases" bind:group={scope} />
					download every database, a Craft one without its personal data
				</label>
			</div>
			<label for="tk-name">Name:</label>
			<!-- svelte-ignore a11y_autofocus -->
			<input
				id="tk-name"
				autofocus
				placeholder="e.g. dmate on my laptop"
				bind:value={name}
			/>
		</Form>
	{:else}
		<ListView
			items={tokens}
			key={t => t.id}
			bind:selected
			columns={[
				{ label: 'Name' },
				{ label: 'May' },
				{ label: 'Owner' },
				{ label: 'Last used' },
			]}
			empty="No tokens yet."
			fill
			minHeight={150}
		>
			{#snippet row(t)}
				<td>{t.name}</td>
				<td>{t.scopes.map(scopeText).join(', ')}</td>
				<td>{t.userName ?? ''}</td>
				<td>{t.lastUsed ? formatTime(t.lastUsed) : 'never'}</td>
			{/snippet}
		</ListView>
		{#if error}<p class="error">{error}</p>{/if}
		{#if created}
			<OneTimeSecret value={created} />
		{/if}
		{#each examples.filter(e => !created || e.scope === createdScope) as e (e.scope)}
			<p>{e.text}</p>
			<pre class="code selectable">{e.code}</pre>
		{/each}
		<p class="hint">Deploy tokens are made in a group's Deploy tab.</p>
		<div class="row-buttons">
			<Button onclick={() => (adding = true)}>New Token…</Button>
			<Button onclick={remove} disabled={selected === null}>
				Delete
			</Button>
			<span class="gap"></span>
			<Button primary onclick={onclose}>Close</Button>
		</div>
	{/if}
</Dialog>

<style lang="scss">
	@use 'styles/win' as *;

	.scopes {
		display: flex;
		flex-direction: column;
		gap: 4px;

		label {
			display: flex;
			gap: 4px;
			align-items: flex-start;
			white-space: normal;
		}
	}

	.label {
		align-self: start;
		padding-top: 2px;
	}

	p {
		margin: 8px 0 4px;
	}
</style>
