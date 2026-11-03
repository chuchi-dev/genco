<script lang="ts">
	import { untrack } from 'svelte';
	import { get, newest, type DnsRecord } from '@/lib/api';
	import { listKeys } from '@/lib/listKeys';
	import Button from './win/Button.svelte';

	// DNS records to set next to what the DNS answers now, from `path`
	// (a mail domain's, genco's own mail); its rows don't select
	let {
		path,
		records = $bindable(null),
	}: { path: string; records?: DnsRecord[] | null } = $props();

	let error = $state<string | null>(null);
	let checking = $state(false);

	const load = newest();
	function check() {
		checking = true;
		load(
			get(path),
			(r: DnsRecord[]) => {
				records = r;
				error = null;
				checking = false;
			},
			e => {
				error = e.message;
				checking = false;
			},
		);
	}
	$effect(() => {
		void path;
		untrack(check);
	});

	const STATUS: Record<DnsRecord['status'], string> = {
		ok: 'ok',
		missing: 'missing',
		differs: 'change',
		optional: 'optional',
	};
</script>

{#if error}<p class="error">{error}</p>{/if}
<div class="list-view" {@attach listKeys}>
	<table>
		<thead>
			<tr>
				<th>Status</th>
				<th>Name</th>
				<th>Type</th>
				<th>Value to set</th>
				<th>Now</th>
			</tr>
		</thead>
		<tbody>
			{#each records ?? [] as r (r.kind + r.name)}
				<tr title={r.note}>
					<td class="dns {r.status}">{STATUS[r.status]}</td>
					<td class="selectable">{r.name}</td>
					<td>{r.kind}</td>
					<td class="selectable mono">{r.value}</td>
					<td class="mono now">{r.found.join(' | ') || '–'}</td>
				</tr>
			{:else}
				<tr><td colspan="5" class="empty">Looking up…</td></tr>
			{/each}
		</tbody>
	</table>
</div>
<div class="row-buttons">
	<Button onclick={check} disabled={checking}>
		{checking ? 'Checking…' : 'Check Again'}
	</Button>
</div>

<style lang="scss">
	@use 'styles/win' as *;

	.list-view {
		max-height: 260px;
		overflow: auto;
	}

	.mono {
		font-family: $mono;
	}

	.now {
		color: $hint;
		max-width: 260px;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.dns {
		font-weight: bold;

		&.ok {
			color: $ok;
		}

		&.missing,
		&.differs {
			color: $error;
		}

		&.optional {
			color: $hint;
			font-weight: normal;
		}
	}
</style>
