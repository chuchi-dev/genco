<script lang="ts">
	import { tree } from '@/lib/tree.svelte';
	import Icon from '@/components/win/Icon.svelte';
	import Backups from '@/components/Backups.svelte';
	import Usage from '@/components/Usage.svelte';
	import Costs from '@/components/Costs.svelte';

	const groups = $derived(tree.groups);
	const domains = $derived(groups.flatMap(g => g.domains).length);
	const databases = $derived(groups.flatMap(g => g.databases).length);
</script>

<div class="head">
	<Icon name="computer" size={32} />
	<h3>{location.hostname}</h3>
</div>
<dl class="props">
	<dt>Clients:</dt>
	<dd>{tree.clients.length}</dd>
	<dt>Groups:</dt>
	<dd>{groups.length}</dd>
	<dt>Domains:</dt>
	<dd>{domains}</dd>
	<dt>Databases:</dt>
	<dd>{databases}</dd>
</dl>

<div class="list-view">
	<table>
		<thead>
			<tr>
				<th>Client</th>
				<th>Contact</th>
				<th>Groups</th>
			</tr>
		</thead>
		<tbody>
			{#each tree.clients as c (c.id)}
				<tr>
					<td><a href="/clients/{c.id}">{c.name}</a></td>
					<td>{c.contact}</td>
					<td>{c.groups.map(g => g.name).join(', ')}</td>
				</tr>
			{:else}
				<tr>
					<td colspan="3" class="empty">
						No clients yet. Use File → New Client, or ask the agent.
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>

<h3 class="section">Usage</h3>
<Usage scope="server" />

<h3 class="section">Agent costs</h3>
<Costs />

<h3 class="section">Backups</h3>
<Backups />

<style>
	.head {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-bottom: 12px;
	}

	h3 {
		margin: 0;
		font-size: 14px;
	}

	.section {
		margin: 18px 0 8px;
	}
</style>
