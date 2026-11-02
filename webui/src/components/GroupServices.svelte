<script lang="ts">
	import { get, newest, type Group, type Unit } from '@/lib/api';
	import { tree } from '@/lib/tree.svelte';

	// the systemd units of a group, again when the tree changes (e.g. after
	// an apply)
	let { group }: { group: Group } = $props();

	let units = $state<Unit[] | null>(null);
	let error = $state<string | null>(null);
	const load = newest();

	$effect(() => {
		void tree.clients;
		load(
			get(`/groups/${group.id}/services`),
			v => ((units = v), (error = null)),
			e => (error = e.message),
		);
	});

	function unitState(u: Unit): string {
		if (u.active === 'active') return 'ok';
		if (u.active === 'failed') return 'bad';
		return 'off';
	}
</script>

<div class="list-view">
	<table>
		<thead>
			<tr>
				<th></th>
				<th>Unit</th>
				<th>State</th>
				<th>Description</th>
			</tr>
		</thead>
		<tbody>
			{#each units ?? [] as u (u.unit)}
				<tr>
					<td><span class="dot {unitState(u)}"></span></td>
					<td>{u.unit}</td>
					<td>{u.active} ({u.sub})</td>
					<td>{u.description}</td>
				</tr>
			{:else}
				<tr>
					<td colspan="4" class="empty">
						{units
							? 'Nothing runs for this group yet.'
							: 'Loading…'}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
{#if error}<p class="error">{error}</p>{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.dot {
		display: inline-block;
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: $shadow;

		&.ok {
			background: $ok;
		}

		&.bad {
			background: $error;
		}
	}
</style>
