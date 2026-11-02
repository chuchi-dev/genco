<script lang="ts">
	import {
		formatBytes,
		get,
		newest,
		type DatabaseInfo,
		type Group,
	} from '@/lib/api';
	import { dialogs } from '@/lib/dialogs.svelte';
	import { tree } from '@/lib/tree.svelte';
	import Button from './win/Button.svelte';

	// a group's databases with their size in MariaDB
	let { group }: { group: Group } = $props();

	let databases = $state<DatabaseInfo[] | null>(null);
	let error = $state<string | null>(null);
	const load = newest();

	$effect(() => {
		void tree.clients;
		load(
			get(`/groups/${group.id}/databases`),
			v => ((databases = v), (error = null)),
			e => (error = e.message),
		);
	});
</script>

<div class="list-view">
	<table>
		<thead>
			<tr>
				<th>Database</th>
				<th>Status</th>
				<th>Size</th>
				<th>Tables</th>
			</tr>
		</thead>
		<tbody>
			{#each databases ?? [] as d (d.name)}
				<tr>
					<td>{d.name}</td>
					<td>{d.exists ? 'exists' : 'not created yet'}</td>
					<td>{d.exists ? formatBytes(d.sizeBytes) : ''}</td>
					<td>{d.exists ? d.tables : ''}</td>
				</tr>
			{:else}
				<tr>
					<td colspan="4" class="empty">
						{databases ? 'No databases yet.' : 'Loading…'}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
{#if error}<p class="error">{error}</p>{/if}
<p class="hint">
	Apps log in as <code>{group.user}</code> over the socket
	<code>/run/mysqld/mysqld.sock</code>, without a password.
</p>
<div class="row-buttons">
	<Button onclick={() => (dialogs.open = 'database')}>New Database…</Button>
</div>

<style>
	.hint {
		margin: 8px 0;
	}
</style>
