<script lang="ts">
	import {
		formatBytes,
		get,
		newest,
		type Sample,
		type Stats,
	} from '@/lib/api';
	import LineChart, { type Series } from './LineChart.svelte';
	import TopPages from './TopPages.svelte';

	// usage over time of the server, a group or a domain
	let {
		scope,
		app = false,
	}: {
		/** server, group:<id> or domain:<id> */
		scope: string;
		/** a domain with a Node or Rust app has its own CPU and memory */
		app?: boolean;
	} = $props();

	const HOUR = 3_600_000;
	const RANGES: [string, string, number][] = [
		['1h', 'Last hour', HOUR],
		['24h', 'Last 24 hours', 24 * HOUR],
		['7d', 'Last 7 days', 7 * 24 * HOUR],
		['30d', 'Last 30 days', 30 * 24 * HOUR],
	];

	let range = $state('24h');
	let stats = $state<Stats | null>(null);
	// the charts show the whole range, also where there is no data yet
	let end = $state(Date.now());
	const start = $derived(end - (RANGES.find(r => r[0] === range)?.[2] ?? 0));
	let error = $state<string | null>(null);

	const load = newest();
	function reload() {
		load(
			get(`/stats?scope=${scope}&range=${range}`),
			(s: Stats) => {
				stats = s;
				end = Date.now();
				error = null;
			},
			e => (error = e.message),
		);
	}
	$effect(() => {
		void scope;
		void range;
		reload();
		// a new point every minute
		const iv = setInterval(reload, 60_000);
		return () => clearInterval(iv);
	});

	const kind = $derived(scope.split(':')[0]);
	const points = $derived(stats?.points ?? []);
	const times = $derived(points.map(p => p.at));
	const period = $derived(stats?.period ?? 60);

	function values(
		f: (p: Sample) => number | null | undefined,
	): (number | null)[] {
		return points.map(p => f(p) ?? null);
	}
	// counters are per period: requests per minute, bytes per second
	const perMinute = (v: number | null) =>
		v === null ? null : (v * 60) / period;
	const perSecond = (v: number | null) => (v === null ? null : v / period);

	const percent = (v: number) => `${Math.round(v)}%`;
	const bytes = (v: number) => (v === 0 ? '0' : formatBytes(Math.round(v)));
	const rate = (v: number) =>
		v === 0 ? '0' : formatBytes(Math.round(v)) + '/s';
	const count = (v: number) =>
		v >= 100 ? String(Math.round(v)) : String(Math.round(v * 10) / 10);

	type Chart = {
		title: string;
		series: Series[];
		format: (v: number) => string;
		max?: number | null;
	};

	const charts = $derived.by((): Chart[] => {
		const cpu: Chart = {
			title: 'CPU (% of all cores)',
			series: [{ name: 'CPU', values: values(p => p.cpu) }],
			format: percent,
			max: kind === 'server' ? 100 : null,
		};
		const memory: Chart = {
			title: 'Memory',
			series: [{ name: 'Memory', values: values(p => p.memory) }],
			format: bytes,
			max: stats?.memoryLimit,
		};
		const disk: Chart = {
			title: kind === 'group' ? 'Disk (files and databases)' : 'Disk',
			series: [{ name: 'Disk', values: values(p => p.disk) }],
			format: bytes,
			max: stats?.diskLimit,
		};
		const requests: Chart = {
			title: 'Requests per minute',
			series: [
				{
					name: 'Requests',
					values: values(p => perMinute(p.requests)),
				},
				{
					name: 'Errors (500+)',
					values: values(p => perMinute(p.errors)),
				},
			],
			format: count,
		};
		const sent: Chart = {
			title: 'Traffic sent',
			series: [{ name: 'Sent', values: values(p => perSecond(p.tx)) }],
			format: rate,
		};
		if (kind === 'server') {
			const network: Chart = {
				title: 'Network',
				series: [
					{ name: 'In', values: values(p => perSecond(p.rx)) },
					{ name: 'Out', values: values(p => perSecond(p.tx)) },
				],
				format: rate,
			};
			return [cpu, memory, network, disk];
		}
		if (kind === 'group') return [requests, sent, cpu, memory, disk];
		return app ? [requests, sent, cpu, memory] : [requests, sent];
	});
</script>

<div class="bar">
	<select aria-label="Time range" bind:value={range}>
		{#each RANGES as [id, label] (id)}
			<option value={id}>{label}</option>
		{/each}
	</select>
	{#if kind === 'group'}
		<span class="hint"
			>CPU and memory of all its PHP and apps together.</span
		>
	{:else if kind === 'domain' && !app}
		<span class="hint">
			CPU and memory of PHP are shared by the group, see its Usage tab.
		</span>
	{/if}
</div>
{#if error}<p class="error">{error}</p>{/if}

<div class="charts">
	{#each charts as c (c.title)}
		<LineChart
			title={c.title}
			{times}
			series={c.series}
			{period}
			format={c.format}
			max={c.max}
			{start}
			{end}
		/>
	{/each}
</div>

{#if stats?.domains.length}
	<h4>Domains, {RANGES.find(r => r[0] === range)?.[1].toLowerCase()}</h4>
	<div class="list-view">
		<table>
			<thead>
				<tr>
					<th>Domain</th>
					<th class="num">Requests</th>
					<th class="num">Errors (500+)</th>
					<th class="num">Sent</th>
				</tr>
			</thead>
			<tbody>
				{#each stats.domains as d (d.name)}
					<tr>
						<td>{d.name}</td>
						<td class="num">{d.requests}</td>
						<td class="num">{d.errors}</td>
						<td class="num">{formatBytes(d.tx)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}

{#if kind === 'domain'}
	<TopPages
		domainId={Number(scope.split(':')[1])}
		hours={(RANGES.find(r => r[0] === range)?.[2] ?? HOUR) / HOUR}
	/>
{/if}

<style lang="scss">
	.bar {
		display: flex;
		gap: 10px;
		align-items: center;
		margin-bottom: 8px;
	}

	.charts {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
		gap: 12px;
	}

	h4 {
		margin: 14px 0 6px;
	}
</style>
