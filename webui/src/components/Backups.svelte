<script lang="ts">
	import {
		formatBytes,
		formatTime,
		get,
		post,
		type Backup,
		type Snapshot,
	} from '@/lib/api';
	import { listen } from '@/lib/events.svelte';
	import Button from './win/Button.svelte';
	import LogView from './LogView.svelte';
	import ListView from './win/ListView.svelte';

	// the backup runs of the server with their logs and the snapshots of
	// the server's config
	let runs = $state<Backup[]>([]);
	let system = $state<Snapshot[]>([]);
	let selected = $state<number | null>(null);
	let error = $state<string | null>(null);

	const shown = $derived(runs.find(r => r.id === selected) ?? null);

	async function load() {
		try {
			[runs, system] = await Promise.all([
				get('/backups'),
				get('/backups/system'),
			]);
		} catch (e: any) {
			error = e.message;
		}
	}
	load();

	const live = $derived(shown?.status === 'running');

	$effect(() =>
		listen(ev => {
			if (ev.type !== 'backup') return;
			load();
		}),
	);

	async function backupNow() {
		error = null;
		try {
			selected = await post('/backups');
			await load();
		} catch (e: any) {
			error = e.message;
		}
	}
</script>

<p>
	Every night genco backs up the files and databases of every group and the
	server's config (<code>/etc</code>, genco's database) with restic, on this
	server for now.
</p>
<div class="row-buttons top">
	<Button onclick={backupNow} disabled={shown?.status === 'running'}>
		Back Up Now
	</Button>
</div>
{#if error}<p class="error">{error}</p>{/if}

<ListView
	items={runs}
	key={r => r.id}
	bind:selected
	columns={[
		{ label: '#' },
		{ label: 'Status' },
		{ label: 'What' },
		{ label: 'By' },
		{ label: 'Started' },
	]}
	empty="No backups yet."
	maxHeight={160}
>
	{#snippet row(r)}
		<td>{r.id}</td>
		<td class="status {r.status}">{r.status}</td>
		<td>{r.groupName ?? 'everything'}</td>
		<td>{r.startedBy}</td>
		<td>{formatTime(r.createdAt)}</td>
	{/snippet}
</ListView>
{#if shown}
	<LogView url="/backups/{shown.id}/log" {live} height={200} />
{/if}

<h4>Server config</h4>
<ListView
	items={system}
	key={s => s.id}
	columns={[{ label: 'Snapshot' }, { label: 'Time' }, { label: 'Size' }]}
	empty="None yet."
	maxHeight={160}
>
	{#snippet row(s)}
		<td><code>{s.id}</code></td>
		<td>{formatTime(Date.parse(s.time))}</td>
		<td>{formatBytes(s.bytes)}</td>
	{/snippet}
</ListView>

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 0 0 8px;
	}

	.top {
		margin: 0 0 8px;
	}

	h4 {
		margin: 14px 0 6px;
		font-size: 12px;
	}
</style>
