<script lang="ts">
	import {
		formatBytes,
		formatTime,
		get,
		newest,
		type TopPages,
	} from '@/lib/api';
	import ListView from './win/ListView.svelte';

	// the most requested paths of a domain, from its access log
	let { domainId, hours }: { domainId: number; hours: number } = $props();

	let top = $state<TopPages | null>(null);
	let error = $state<string | null>(null);

	// reads the log, so only when the period changes, not every minute
	const load = newest();
	$effect(() => {
		load(
			get(`/domains/${domainId}/top?hours=${hours}`),
			(t: TopPages) => {
				top = t;
				error = null;
			},
			e => (error = e.message),
		);
	});
</script>

<!-- one block: in the page's flex column the scrolling lists would
	shrink to nothing -->
<div>
	<h4>Top pages</h4>
	{#if error}<p class="error">{error}</p>{/if}
	{#if top}
		{#if top.from}
			<p class="hint">
				The log only goes back to {formatTime(top.from)}.
			</p>
		{/if}
		<ListView
			items={top.pages}
			key={p => p.path}
			columns={[
				{ label: 'Path' },
				{ label: 'Requests', num: true },
				{ label: '4xx', num: true },
				{ label: '5xx', num: true },
				{ label: 'Sent', num: true },
			]}
			empty="No requests."
			maxHeight={240}
		>
			{#snippet row(p)}
				<td class="path">{p.path}</td>
				<td class="num">{p.requests}</td>
				<td class="num">{p.clientErrors}</td>
				<td class="num">{p.serverErrors}</td>
				<td class="num">{formatBytes(p.bytes)}</td>
			{/snippet}
		</ListView>

		<h4>Not found (404)</h4>
		<ListView
			items={top.notFound}
			key={p => p.path}
			columns={[{ label: 'Path' }, { label: 'Requests', num: true }]}
			empty="None."
			maxHeight={180}
		>
			{#snippet row(p)}
				<td class="path">{p.path}</td>
				<td class="num">{p.requests}</td>
			{/snippet}
		</ListView>

		<h4>Top addresses</h4>
		<ListView
			items={top.addresses}
			key={a => a.ip}
			columns={[
				{ label: 'Address' },
				{ label: 'Requests', num: true },
				{ label: '4xx', num: true },
				{ label: '5xx', num: true },
				{ label: 'Most requested' },
			]}
			empty="None."
			maxHeight={180}
		>
			{#snippet row(a)}
				<td>{a.ip}</td>
				<td class="num">{a.requests}</td>
				<td class="num">{a.clientErrors}</td>
				<td class="num">{a.serverErrors}</td>
				<td class="path">{a.path}</td>
			{/snippet}
		</ListView>
	{/if}
</div>

<style>
	h4 {
		margin: 14px 0 6px;
	}

	.path {
		word-break: break-all;
	}
</style>
