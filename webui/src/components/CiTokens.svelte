<script lang="ts">
	import OneTimeSecret from './OneTimeSecret.svelte';
	import { formatTime, get, post, type Group, type Token } from '@/lib/api';
	import { message, confirmDelete } from '@/lib/dialogs.svelte';
	import Button from './win/Button.svelte';
	import ListView from './win/ListView.svelte';

	// the tokens CI jobs deploy a group with, and a GitLab example
	let { group, branch }: { group: Group; branch: string } = $props();

	let tokens = $state<Token[]>([]);
	let newToken = $state<string | null>(null);
	let error = $state<string | null>(null);

	const scope = $derived(`deploy:${group.id}`);
	const groupTokens = $derived(tokens.filter(t => t.scopes.includes(scope)));

	async function load() {
		try {
			tokens = await get('/tokens');
		} catch (e: any) {
			error = e.message;
		}
	}
	load();

	async function addToken() {
		const name = await message.ask(
			'New Token',
			'Name of the token, e.g. "GitLab CI":',
		);
		if (!name) return;
		try {
			const password = await message.password(
				'Deploy Token',
				'create a deploy token',
			);
			if (password === null) return;
			newToken = await post('/tokens', {
				name,
				scopes: [scope],
				password,
			});
			await load();
		} catch (e: any) {
			error = e.message;
		}
	}

	async function removeToken(t: Token) {
		const deleted = await confirmDelete({
			title: 'Delete Token',
			text: `Delete the token ${t.name}? CI jobs using it stop working.`,
			path: '/tokens/' + t.id,
		});
		if (deleted) await load();
	}

	const ciExample = $derived(
		`deploy:\n  stage: deploy\n  script:\n    - >\n      curl -fsS -X POST ${location.origin}/api/deploy\n      -H "Authorization: Bearer $GENCO_TOKEN"\n      -H "content-type: application/json"\n      -d "{\\"group\\": \\"${group.name}\\", \\"ref\\": \\"$CI_COMMIT_SHA\\"}"\n  rules:\n    - if: $CI_COMMIT_BRANCH == "${branch}"`,
	);
</script>

<fieldset>
	<legend>Deploy from CI</legend>
	<p>
		A CI job deploys with a token that may only deploy this group. Store it
		as the masked variable <code>GENCO_TOKEN</code>.
	</p>
	<ListView
		items={groupTokens}
		key={t => t.id}
		columns={[
			{ label: 'Token' },
			{ label: 'Created' },
			{ label: 'Last used' },
			{ label: '' },
		]}
		empty="No tokens yet."
		maxHeight={120}
		class="ci-tokens"
	>
		{#snippet row(t)}
			<td>{t.name}</td>
			<td>{formatTime(t.createdAt)}</td>
			<td>{t.lastUsed ? formatTime(t.lastUsed) : 'never'}</td>
			<td class="num">
				<button class="link" onclick={() => removeToken(t)}
					>Delete</button
				>
			</td>
		{/snippet}
	</ListView>
	{#if error}<p class="error">{error}</p>{/if}
	{#if newToken}
		<OneTimeSecret value={newToken} />
	{/if}
	<div class="row-buttons">
		<Button onclick={addToken}>New Token…</Button>
	</div>
	<p>
		For GitLab, in <code>.gitlab-ci.yml</code>:
	</p>
	<pre class="code selectable">{ciExample}</pre>
</fieldset>

<style lang="scss">
	:global(.list-view.ci-tokens) {
		margin-bottom: 6px;
	}
</style>
