<script lang="ts">
	import {
		formatTime,
		get,
		post,
		type Deploy,
		type Group,
		saveGroup,
	} from '@/lib/api';
	import { message } from '@/lib/dialogs.svelte';
	import { listen } from '@/lib/events.svelte';
	import Button from './win/Button.svelte';
	import LogView from './LogView.svelte';
	import CiTokens from './CiTokens.svelte';
	import Form from './win/Form.svelte';
	import ListView from './win/ListView.svelte';

	// the group's repository, its deploys with their logs and the tokens
	// CI jobs deploy with
	let { group }: { group: Group } = $props();

	// svelte-ignore state_referenced_locally
	let repo = $state(group.repo ?? '');
	// svelte-ignore state_referenced_locally
	let branch = $state(group.branch);
	let error = $state<string | null>(null);

	let key = $state<string | null>(null);
	let deploys = $state<Deploy[]>([]);
	let selected = $state<number | null>(null);
	let gitRef = $state('');
	let busy = $state(false);

	// the server reads which release runs, also after a failed step
	const current = $derived(deploys.find(d => d.live)?.id);
	const shown = $derived(deploys.find(d => d.id === selected) ?? null);

	async function load() {
		try {
			[key, deploys] = await Promise.all([
				get(`/groups/${group.id}/deploy-key`),
				get(`/groups/${group.id}/deploys`),
			]);
			if (selected === null && deploys.length) selected = deploys[0].id;
		} catch (e: any) {
			error = e.message;
		}
	}
	load();

	const live = $derived(
		shown?.status === 'running' || shown?.status === 'queued',
	);

	$effect(() =>
		listen(ev => {
			if (ev.type !== 'deploy' || ev.deploy.groupId !== group.id) return;
			const i = deploys.findIndex(d => d.id === ev.deploy.id);
			if (i >= 0) deploys[i] = { ...ev.deploy, live: deploys[i].live };
			else deploys = [ev.deploy, ...deploys];
			// a finished deploy may have switched the live release
			if (ev.deploy.status === 'done' || ev.deploy.status === 'failed')
				load();
		}),
	);

	async function save() {
		await saveGroup(group, { repo, branch });
	}

	async function deploy() {
		busy = true;
		error = null;
		try {
			selected = await post(`/groups/${group.id}/deploy`, {
				ref: gitRef.trim() || null,
			});
			await load();
		} catch (e: any) {
			error = e.message;
		}
		busy = false;
	}

	async function rollback(d: Deploy) {
		if (
			!(await message.confirm(
				'Roll Back',
				`Switch back to the release of #${d.id} (${d.ref})? The database stays as it is.`,
			))
		)
			return;
		try {
			selected = await post(`/groups/${group.id}/rollback`, {
				deployId: d.id,
			});
			await load();
		} catch (e: any) {
			error = e.message;
		}
	}

	async function cancel(d: Deploy) {
		if (
			!(await message.confirm(
				'Cancel Deploy',
				`Take the build of #${d.id} out of the runners' queue? The deploy fails, the live site stays.`,
			))
		)
			return;
		try {
			await post(`/deploys/${d.id}/cancel`, {});
		} catch (e: any) {
			error = e.message;
		}
	}

	// a running deploy whose build waits for a runner shows as queued
	function statusLabel(d: Deploy): string {
		if (d.queue) return `queued, #${d.queue} for a runner`;
		return d.status + (d.id === current ? ', live' : '');
	}

	// a commit as ref is shown short, a branch with its commit
	function refLabel(d: Deploy): string {
		if (/^[0-9a-f]{40}$/.test(d.ref)) return d.ref.slice(0, 8);
		return d.commitId ? `${d.ref} (${d.commitId.slice(0, 8)})` : d.ref;
	}
</script>

<Form onsubmit={save} ok="Save" done="Saved">
	<label for="dp-repo">Repository:</label>
	<input
		id="dp-repo"
		placeholder="git@git.example.ch:team/project.git"
		bind:value={repo}
	/>
	<label for="dp-branch">Branch:</label>
	<input id="dp-branch" bind:value={branch} />
</Form>

{#if group.repo}
	<fieldset>
		<legend>Deploy key</legend>
		{#if key}
			<p>Add it to the repository as a read-only deploy key:</p>
			<pre class="code selectable">{key}</pre>
		{:else}
			<p>Apply the changes (Server → Apply Changes) to create it.</p>
		{/if}
	</fieldset>

	{#if error}<p class="error">{error}</p>{/if}
	<div class="deploy-bar">
		<input
			placeholder="branch, tag or commit (default {group.branch})"
			aria-label="Branch, tag or commit to deploy"
			bind:value={gitRef}
		/>
		<Button primary onclick={deploy} disabled={busy}>Deploy now</Button>
	</div>

	<div class="split">
		<ListView
			items={deploys}
			key={d => d.id}
			bind:selected
			columns={[
				{ label: '#' },
				{ label: 'Ref' },
				{ label: 'Status' },
				{ label: 'By' },
				{ label: 'When' },
			]}
			empty="No deploys yet."
			maxHeight={180}
		>
			{#snippet row(d)}
				<td>{d.id}</td>
				<td>{refLabel(d)}</td>
				<td class="status {d.queue ? 'queued' : d.status}">
					{statusLabel(d)}
				</td>
				<td>{d.startedBy}</td>
				<td>{formatTime(d.createdAt)}</td>
			{/snippet}
		</ListView>
		{#if shown}
			<div class="log-head">
				<span>Log of #{shown.id}</span>
				{#if shown.queue}
					<Button onclick={() => cancel(shown)}>Cancel</Button>
				{/if}
				{#if shown.status === 'done' && shown.id !== current && shown.release}
					<Button onclick={() => rollback(shown)}>
						Roll back to this
					</Button>
				{/if}
			</div>
			{#if shown.status === 'failed' && shown.id === current}
				<p class="error">
					This release is live: a step after the switch failed, see
					the end of the log.
				</p>
			{/if}
			<LogView url="/deploys/{shown.id}/log" {live} />
		{/if}
	</div>

	<CiTokens {group} {branch} />
{:else}
	<p class="hint">Save a repository to deploy this group from git.</p>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.deploy-bar {
		display: flex;
		gap: 6px;
		margin: 14px 0 6px;

		input {
			flex: 1;
		}
	}

	.log-head {
		display: flex;
		justify-content: space-between;
		align-items: center;
		margin: 8px 0 4px;
	}

	.hint {
		margin: 12px 0;
	}
</style>
