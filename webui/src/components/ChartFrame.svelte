<script lang="ts" module>
	// the series' colors as a line or a box, for a legend and a tooltip
	export { legend, swatch };

	type Shape = 'line' | 'box';
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import { PAD, yScale } from '@/lib/chart';

	// the sunken plot of a chart: the y grid with its labels around what
	// the chart draws, and a tooltip
	let {
		width = $bindable(),
		height,
		left,
		top,
		format,
		label,
		empty,
		tipLeft = null,
		tipWidth,
		onpointermove,
		onpointerleave,
		children,
		tip,
	}: {
		width: number;
		height: number;
		/** room for the y labels */
		left: number;
		/** the value at the top of the scale */
		top: number;
		format: (v: number) => string;
		label: string;
		/** shown instead of the plot, e.g. without data */
		empty?: string;
		/** where the tooltip is, null hides it */
		tipLeft?: number | null;
		tipWidth: number;
		onpointermove?: (e: PointerEvent) => void;
		onpointerleave: () => void;
		/** what the chart draws, inside the svg */
		children: Snippet;
		tip?: Snippet;
	} = $props();

	const plotW = $derived(Math.max(10, width - left - PAD.right));
	const y = $derived(yScale(top, height));
</script>

{#snippet swatch(i: number, shape: Shape)}
	<i class="{shape} s{i + 1}"></i>
{/snippet}

{#snippet legend(names: readonly string[], shape: Shape)}
	<span class="legend">
		{#each names as name, i (name)}
			<span class:key={shape === 'line'}
				>{@render swatch(i, shape)}{name}</span
			>
		{/each}
	</span>
{/snippet}

<div class="plot" style:height="{height}px" bind:clientWidth={width}>
	{#if empty}
		<p class="empty">{empty}</p>
	{:else}
		<svg
			{width}
			{height}
			role="img"
			aria-label={label}
			style:touch-action={onpointermove ? 'pan-y' : undefined}
			{onpointermove}
			{onpointerleave}
		>
			{#each [0, 0.5, 1] as f (f)}
				<line
					class="grid"
					x1={left}
					x2={left + plotW}
					y1={y(top * f)}
					y2={y(top * f)}
				/>
				<text
					class="axis"
					x={left - 4}
					y={y(top * f) + 4}
					text-anchor="end"
				>
					{format(top * f)}
				</text>
			{/each}
			{@render children()}
		</svg>
		{#if tipLeft !== null && tip}
			<div
				class="tip"
				style:left="{tipLeft}px"
				style:min-width="{tipWidth}px"
			>
				{@render tip()}
			</div>
		{/if}
	{/if}
</div>

<style lang="scss">
	@use 'styles/win' as *;

	.legend {
		display: flex;
		gap: 10px;
		color: $hint;
	}

	.key {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.line,
	.box {
		display: inline-block;
		margin-right: 4px;

		@each $i, $color in (1: $series-1, 2: $series-2, 3: $series-3) {
			&.s#{$i} {
				background: $color;
			}
		}
	}

	.line {
		width: 12px;
		height: 2px;
		vertical-align: middle;
	}

	.box {
		width: 10px;
		height: 10px;
		vertical-align: -1px;
		border-radius: 2px;
	}

	.plot {
		@include sunken;
		position: relative;
		background: $white;
	}

	svg {
		display: block;
	}

	.grid {
		stroke: #e4e4e4;
		stroke-width: 1;
	}

	// also the x labels the chart draws
	.plot :global(.axis) {
		font-size: 10px;
		fill: $hint;
	}

	.tip {
		position: absolute;
		top: 6px;
		padding: 3px 6px;
		background: #ffffe1;
		border: 1px solid $dark;
		pointer-events: none;
		white-space: nowrap;

		:global(.when) {
			color: $hint;
		}
	}

	.empty {
		margin: 0;
		padding: 46px 0;
		text-align: center;
		color: $hint;
	}
</style>
