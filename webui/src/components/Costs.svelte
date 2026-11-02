<script lang="ts">
	import { get } from '@/lib/api';
	import { formatUsd as usd } from '@/lib/format';
	import { niceMax, PAD, yScale } from '@/lib/chart';
	import ChartFrame, { legend, swatch } from './ChartFrame.svelte';

	// what the agent cost per day, split into the admins' conversations,
	// routines and alerts: stacked bars, the same numbers as a table
	type Day = {
		date: string;
		chat: number;
		routine: number;
		alert: number;
		requests: number;
		tokens: number;
		unpriced: number;
	};

	const DAYS = 30;
	const KINDS = [
		['chat', 'Conversations'],
		['routine', 'Routines'],
		['alert', 'Alerts'],
	] as const;

	let days = $state<Day[]>([]);
	// the server's date: its days are in its time zone, not the browser's
	let today = $state<string | null>(null);
	let error = $state<string | null>(null);
	let hover = $state<number | null>(null);
	let width = $state(600);
	let showTable = $state(false);

	get(`/costs?days=${DAYS}`)
		.then(r => {
			days = r.days;
			today = r.today;
		})
		.catch(e => (error = e.message));

	// every day of the range, also those without a request
	const slots = $derived.by(() => {
		const byDate = new Map(days.map(d => [d.date, d]));
		const out: Day[] = [];
		if (!today) return out;
		// calendar days back from the server's today, noon in UTC is the
		// same date everywhere and has no daylight saving step
		const [y, m, d] = today.split('-').map(Number);
		for (let i = DAYS - 1; i >= 0; i--) {
			const t = new Date(Date.UTC(y, m - 1, d - i, 12));
			const date = t.toISOString().slice(0, 10);
			out.push(
				byDate.get(date) ?? {
					date,
					chat: 0,
					routine: 0,
					alert: 0,
					requests: 0,
					tokens: 0,
					unpriced: 0,
				},
			);
		}
		return out;
	});

	const total = (d: Day) => d.chat + d.routine + d.alert;
	const sum = (n: number) =>
		slots.slice(-n).reduce((s, d) => s + total(d), 0);

	const H = 140;
	const LEFT = 48;
	const plotW = $derived(Math.max(10, width - LEFT - PAD.right));
	const plotH = H - PAD.top - PAD.bottom;
	const top = $derived(niceMax(Math.max(0.01, ...slots.map(total))));
	const slot = $derived(plotW / DAYS);
	const barW = $derived(Math.max(2, slot - 3));
	const y = $derived(yScale(top, H));

	/** the segments of a day from the bottom, 2px apart */
	function segments(d: Day) {
		let base = 0;
		return KINDS.map(([k], i) => {
			const v = d[k];
			const seg = { i, y0: y(base), y1: y(base + v), v };
			base += v;
			return seg;
		}).filter(s => s.v > 0);
	}

	function label(date: string): string {
		const [, m, d] = date.split('-');
		return `${d}.${m}.`;
	}
</script>

{#if error}<p class="error">{error}</p>{/if}
<p class="totals">
	Today <b>{usd(sum(1))}</b> · last 7 days <b>{usd(sum(7))}</b> · last 30 days
	<b>{usd(sum(30))}</b>
	{@render legend(
		KINDS.map(([, name]) => name),
		'box',
	)}
</p>
<ChartFrame
	bind:width
	height={H}
	left={LEFT}
	{top}
	format={usd}
	label="Costs of the agent per day"
	tipLeft={hover === null
		? null
		: Math.min(LEFT + hover * slot + 8, width - 170)}
	tipWidth={150}
	onpointerleave={() => (hover = null)}
>
	{#each slots as d, i (d.date)}
		{@const x = LEFT + i * slot + (slot - barW) / 2}
		<!-- the whole slot answers the pointer, not only the bar -->
		<rect
			class="hit"
			x={LEFT + i * slot}
			y={PAD.top}
			width={slot}
			height={plotH}
			role="presentation"
			onpointerenter={() => (hover = i)}
		/>
		{#each segments(d) as s (s.i)}
			<rect
				class="bar s{s.i + 1}"
				class:lit={hover === i}
				{x}
				y={s.y1}
				width={barW}
				height={Math.max(1, s.y0 - s.y1 - 2)}
				rx="1"
			/>
		{/each}
		{#if i % 7 === (DAYS % 7) - 1 || i === DAYS - 1}
			<text class="axis" x={x + barW / 2} y={H - 4} text-anchor="middle">
				{label(d.date)}
			</text>
		{/if}
	{/each}

	{#snippet tip()}
		{#if hover !== null}
			{@const d = slots[hover]}
			<div class="when">{label(d.date)} · {d.requests} requests</div>
			<div><b>{usd(total(d))}</b> in total</div>
			{#each KINDS as [k, name], i (k)}
				{#if d[k] > 0}
					<div>
						{@render swatch(i, 'box')}<b>{usd(d[k])}</b>
						{name}
					</div>
				{/if}
			{/each}
			{#if d.unpriced}<div class="when">
					{d.unpriced} without a price
				</div>{/if}
		{/if}
	{/snippet}
</ChartFrame>

<button class="link" onclick={() => (showTable = !showTable)}>
	{showTable ? 'Hide the table' : 'Show as a table'}
</button>
{#if showTable}
	<div class="list-view table">
		<table>
			<thead>
				<tr>
					<th>Day</th>
					{#each KINDS as [k, name] (k)}<th class="num">{name}</th
						>{/each}
					<th class="num">Total</th>
					<th class="num">Requests</th>
					<th class="num">Tokens</th>
				</tr>
			</thead>
			<tbody>
				{#each [...slots]
					.reverse()
					.filter(d => d.requests) as d (d.date)}
					<tr>
						<td>{label(d.date)}</td>
						{#each KINDS as [k] (k)}<td class="num">{usd(d[k])}</td
							>{/each}
						<td class="num"><b>{usd(total(d))}</b></td>
						<td class="num">{d.requests}</td>
						<td class="num">{d.tokens.toLocaleString()}</td>
					</tr>
				{:else}
					<tr><td colspan="7" class="empty">No requests yet.</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}
<p class="hint">
	At Anthropic's list prices, counted since genco records them. Requests going
	through OpenRouter may cost a little more.
</p>

<style lang="scss">
	@use 'styles/win' as *;

	.totals {
		display: flex;
		flex-wrap: wrap;
		gap: 4px 14px;
		align-items: baseline;
		margin: 0 0 6px;
	}

	.hit {
		fill: transparent;
	}

	.bar {
		pointer-events: none;

		@each $i, $color in (1: $series-1, 2: $series-2, 3: $series-3) {
			&.s#{$i} {
				fill: $color;
			}
		}

		&.lit {
			opacity: 0.8;
		}
	}

	.link {
		margin: 6px 0 0;
	}

	.table {
		margin-top: 6px;
		max-height: 240px;
	}

	.hint {
		margin: 6px 0 0;
	}
</style>
