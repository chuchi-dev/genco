<script lang="ts">
	import {
		formatBytes,
		formatTime,
		get,
		newest,
		post,
		type Backup,
		type BackupSettings,
		type Snapshot,
	} from '@/lib/api';
	import { listen } from '@/lib/events.svelte';
	import Button from './win/Button.svelte';
	import LogView from './LogView.svelte';
	import ListView from './win/ListView.svelte';

	// the backup runs of the server with their logs and the snapshots of
	// the server's config
	let runs = $state<Backup[]>([]);
	let system = $state<Snapshot[] | null>(null);
	let systemError = $state<string | null>(null);
	let selected = $state<number | null>(null);
	let error = $state<string | null>(null);
	let settings = $state<BackupSettings | null>(null);
	// the server config's snapshots here or in the remote copy
	let remote = $state(false);

	const shown = $derived(runs.find(r => r.id === selected) ?? null);

	async function load() {
		try {
			[runs, settings] = await Promise.all([
				get('/backups'),
				get('/settings/backup'),
			]);
		} catch (e: any) {
			error = e.message;
		}
	}

	// on its own: the remote may take up to a minute to answer
	const latest = newest();
	function loadSystem() {
		system = null;
		systemError = null;
		latest(
			get('/backups/system' + (remote ? '?remote=true' : '')),
			s => (system = s),
			e => (systemError = e.message),
		);
	}
	load();
	loadSystem();

	const live = $derived(shown?.status === 'running');

	$effect(() =>
		listen(ev => {
			if (ev.type !== 'backup') return;
			load();
			// the remote changes after the copy, not on every step
			if (!remote || ev.backup.status !== 'running') loadSystem();
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
	server{settings?.remote
		? ' and as a copy in ' + settings.remote.location
		: ' only (a copy off the server: Start → Settings → Backups)'}.
</p>
{#if settings?.remote}
	<p class:error={!!settings.state.lastErrorAt}>
		Last remote copy: {settings.state.lastOk
			? formatTime(settings.state.lastOk)
			: 'never'}{settings.state.size !== null
			? ', ' + formatBytes(settings.state.size) + ' in the bucket'
			: ''}{settings.state.lastErrorAt
			? '; the last one failed: ' + settings.state.lastError
			: ''}
	</p>
{/if}
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
	minHeight={110}
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

<div class="config">
	<h4>Server config</h4>
	{#if settings?.remote}
		<select
			aria-label="Repository"
			bind:value={remote}
			onchange={loadSystem}
		>
			<option value={false}>On this server</option>
			<option value={true}>Remote copy</option>
		</select>
	{/if}
</div>
<ListView
	items={system ?? []}
	key={s => s.id}
	columns={[{ label: 'Snapshot' }, { label: 'Time' }, { label: 'Size' }]}
	empty={system ? 'No snapshots yet.' : systemError ? '' : 'Loading…'}
	minHeight={110}
	maxHeight={160}
>
	{#snippet row(s)}
		<td><code>{s.id}</code></td>
		<td>{formatTime(Date.parse(s.time))}</td>
		<td>{formatBytes(s.bytes)}</td>
	{/snippet}
</ListView>
{#if systemError}<p class="error">{systemError}</p>{/if}

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 0 0 8px;
	}

	.top {
		margin: 0 0 8px;
	}

	.config {
		display: flex;
		gap: 8px;
		align-items: center;
		margin: 14px 0 6px;
	}

	h4 {
		margin: 0;
		font-size: 12px;
	}
</style>
