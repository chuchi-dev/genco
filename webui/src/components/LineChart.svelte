<script lang="ts" module>
	export type Series = {
		name: string;
		/** one value per time, null where there is none */
		values: (number | null)[];
	};
</script>

<script lang="ts">
	import { formatHour, formatTime } from '@/lib/format';
	import { niceMax, PAD, yScale } from '@/lib/chart';
	import ChartFrame, { legend, swatch } from './ChartFrame.svelte';

	// a line chart over time in a sunken box: one or two series, a
	// crosshair with every value under the pointer, gaps where data is
	// missing (genco was down, an app stopped)
	let {
		title,
		times,
		series,
		period,
		format,
		max,
		start,
		end,
	}: {
		title: string;
		times: number[];
		series: Series[];
		/** seconds between two points, a longer step is a gap */
		period: number;
		format: (v: number) => string;
		/** top of the scale, e.g. the memory of the server */
		max?: number | null;
		/** the time range shown, the data's own by default */
		start?: number;
		end?: number;
	} = $props();

	const H = 120;
	const LEFT = 56;

	let width = $state(300);
	let hover = $state<number | null>(null);
	// new data, the index under the pointer means another time
	$effect(() => {
		void times;
		hover = null;
	});

	const plotW = $derived(Math.max(10, width - LEFT - PAD.right));
	const plotH = H - PAD.top - PAD.bottom;
	const t0 = $derived(start ?? times[0] ?? 0);
	const t1 = $derived(end ?? times.at(-1) ?? 1);

	const top = $derived.by(() => {
		const highest = Math.max(
			0,
			...series.flatMap(s => s.values.filter(v => v !== null)),
		);
		return niceMax(Math.max(highest, max ?? 0) || 1);
	});

	function x(t: number): number {
		return LEFT + (t1 === t0 ? 0 : ((t - t0) / (t1 - t0)) * plotW);
	}

	const y = $derived(yScale(top, H));

	/** a path that breaks at missing values and at gaps in time */
	function path(values: (number | null)[]): string {
		let d = '';
		let open = false;
		values.forEach((v, i) => {
			const gap = i > 0 && times[i] - times[i - 1] > period * 1500;
			if (v === null || gap) open = false;
			if (v === null) return;
			const xy = `${x(times[i]).toFixed(1)},${y(v).toFixed(1)}`;
			// a zero length piece: a lone point shows as a dot (round caps)
			d += open ? `L${xy}` : `M${xy}l0,0`;
			open = true;
		});
		return d;
	}

	function timeLabel(t: number): string {
		// over two days the day too
		return t1 - t0 > 2 * 86_400_000 ? formatTime(t, true) : formatHour(t);
	}

	function onpointermove(e: PointerEvent) {
		if (!times.length) return;
		const rect = (e.currentTarget as SVGElement).getBoundingClientRect();
		const t = t0 + ((e.clientX - rect.left - LEFT) / plotW) * (t1 - t0);
		let best = 0;
		times.forEach((tt, i) => {
			if (Math.abs(tt - t) < Math.abs(times[best] - t)) best = i;
		});
		hover = best;
	}
</script>

<figure class="chart">
	<figcaption>
		<b>{title}</b>
		{#if series.length > 1}
			{@render legend(
				series.map(s => s.name),
				'line',
			)}
		{/if}
	</figcaption>
	<ChartFrame
		bind:width
		height={H}
		left={LEFT}
		{top}
		{format}
		label={title}
		empty={times.length < 2
			? 'No data yet, genco samples every minute.'
			: undefined}
		tipLeft={hover === null
			? null
			: Math.min(x(times[hover]) + 8, width - 150)}
		tipWidth={110}
		{onpointermove}
		onpointerleave={() => (hover = null)}
	>
		{#each [t0, (t0 + t1) / 2, t1] as t, i (i)}
			<text
				class="axis"
				x={x(t)}
				y={H - 4}
				text-anchor={i === 0 ? 'start' : i === 2 ? 'end' : 'middle'}
			>
				{timeLabel(t)}
			</text>
		{/each}
		{#each series as s, i (s.name)}
			<path class="series s{i + 1}" d={path(s.values)} />
		{/each}
		{#if hover !== null}
			<line
				class="crosshair"
				x1={x(times[hover])}
				x2={x(times[hover])}
				y1={PAD.top}
				y2={PAD.top + plotH}
			/>
			{#each series as s, i (s.name)}
				{#if s.values[hover] !== null}
					<circle
						class="dot s{i + 1}"
						cx={x(times[hover])}
						cy={y(s.values[hover]!)}
						r="4"
					/>
				{/if}
			{/each}
		{/if}

		{#snippet tip()}
			{#if hover !== null}
				<div class="when">{timeLabel(times[hover])}</div>
				{#each series as s, i (s.name)}
					<div>
						{@render swatch(i, 'line')}
						<b
							>{s.values[hover] === null
								? '–'
								: format(s.values[hover]!)}</b
						>
						{#if series.length > 1}{s.name}{/if}
					</div>
				{/each}
			{/if}
		{/snippet}
	</ChartFrame>
</figure>

<style lang="scss">
	@use 'styles/win' as *;

	.chart {
		margin: 0;
		min-width: 0;
	}

	figcaption {
		display: flex;
		gap: 10px;
		align-items: baseline;
		margin-bottom: 3px;
	}

	.series {
		fill: none;
		stroke-width: 2;
		stroke-linejoin: round;
		stroke-linecap: round;
	}

	.crosshair {
		stroke: $shadow;
		stroke-width: 1;
	}

	.dot {
		stroke: $white;
		stroke-width: 2;
	}

	@each $i, $color in (1: $series-1, 2: $series-2) {
		.series.s#{$i} {
			stroke: $color;
		}

		.dot.s#{$i} {
			fill: $color;
		}
	}
</style>
