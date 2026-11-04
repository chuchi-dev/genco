<script lang="ts">
	import {
		formatBytes,
		formatTime,
		get,
		post,
		snapshotKind,
		type Backup,
		type Group,
		type Snapshot,
	} from '@/lib/api';
	import { listen } from '@/lib/events.svelte';
	import Button from './win/Button.svelte';
	import LogView from './LogView.svelte';
	import ListView from './win/ListView.svelte';

	// a group's snapshots: its files and a dump per database
	let { group }: { group: Group } = $props();

	let snapshots = $state<Snapshot[] | null>(null);
	let error = $state<string | null>(null);
	// a backup of this group started here, its log shows below
	let run = $state<Backup | null>(null);
	// the snapshots here or in the copy off the server, when there is one
	let hasRemote = $state(false);
	let remote = $state(false);
	get('/settings/backup')
		.then(s => (hasRemote = !!s.remote))
		.catch(() => {});

	// the tree gives a new group object on every change, only its id
	// decides
	const groupId = $derived(group.id);
	function load() {
		snapshots = null;
		get(`/groups/${groupId}/snapshots` + (remote ? '?remote=true' : ''))
			.then(s => (snapshots = s))
			.catch(e => (error = e.message));
	}
	$effect(() => {
		load();
	});

	$effect(() =>
		listen(ev => {
			if (ev.type !== 'backup' || ev.backup.id !== run?.id) return;
			run = ev.backup;
			if (run.status !== 'running') load();
		}),
	);

	async function backupNow() {
		error = null;
		try {
			const id: number = await post(`/groups/${groupId}/backup`);
			// the backup events bring the rest
			run = {
				id,
				status: 'running',
				startedBy: '',
				createdAt: Date.now(),
				finishedAt: null,
				groupId,
				groupName: group.name,
			};
		} catch (e: any) {
			error = e.message;
		}
	}
</script>

<div class="row-buttons top">
	<Button onclick={backupNow} disabled={run?.status === 'running'}>
		Back Up Now
	</Button>
	{#if hasRemote}
		<select aria-label="Repository" bind:value={remote} onchange={load}>
			<option value={false}>On this server</option>
			<option value={true}>Remote copy</option>
		</select>
	{/if}
	{#if run}
		<span class="status {run.status}">
			{run.status === 'running' ? 'Backing up…' : run.status}
		</span>
	{/if}
</div>
{#if run}
	<LogView
		url="/backups/{run.id}/log"
		live={run.status === 'running'}
		height={140}
	/>
{/if}

<ListView
	items={snapshots ?? []}
	key={s => s.id}
	columns={[
		{ label: 'Snapshot' },
		{ label: 'What' },
		{ label: 'Time' },
		{ label: 'Size' },
	]}
	empty={snapshots ? 'No backups yet.' : 'Loading…'}
	minHeight={110}
	maxHeight={260}
>
	{#snippet row(s)}
		<td><code>{s.id}</code></td>
		<td>{snapshotKind(s)}</td>
		<td>{formatTime(Date.parse(s.time))}</td>
		<td>{formatBytes(s.bytes)}</td>
	{/snippet}
</ListView>
{#if error}<p class="error">{error}</p>{/if}
<p class="hint">
	Kept here: the last 3, 7 daily, 4 weekly and 6 monthly{hasRemote
		? ', the remote copy keeps its own history'
		: ''}. To restore files or a database, ask the agent, e.g. "restore the
	database {group.databases[0]?.name ?? group.name} of yesterday into a new database".
</p>

<style lang="scss">
	@use 'styles/win' as *;

	.hint {
		margin: 8px 0;
		color: $hint;
	}

	.row-buttons.top {
		margin: 0 0 8px;
		align-items: center;
	}
</style>
