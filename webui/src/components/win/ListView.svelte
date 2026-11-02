<script lang="ts" generics="T, K extends string | number">
	import type { Snippet } from 'svelte';
	import { listKeys } from '@/lib/listKeys';

	// a list with column headers like the Explorer's details view, the
	// styles are the global .list-view in app.scss
	let {
		items,
		key,
		selected = $bindable(),
		columns,
		empty,
		ondblclick,
		row,
		fill = false,
		minHeight,
		maxHeight,
		class: cls = '',
	}: {
		items: T[];
		key: (item: T) => K;
		/// left out, the rows don't select (a read-only list)
		selected?: K | null;
		columns: { label: string; num?: boolean }[];
		/// the text of the row when there are no items, none without
		empty?: string;
		/// also on Enter (lib/listKeys)
		ondblclick?: (item: T) => void;
		/// the cells of one row
		row: Snippet<[T]>;
		/// takes the free height of a resizable dialog
		fill?: boolean;
		minHeight?: number;
		maxHeight?: number;
		class?: string;
	} = $props();

	// read-only lists stay out of the Tab order, their keys would do nothing
	const selectable = $derived(selected !== undefined);
</script>

<div
	class="list-view {cls}"
	class:fill
	style:min-height={minHeight && `${minHeight}px`}
	style:max-height={maxHeight && `${maxHeight}px`}
	{@attach selectable && listKeys}
>
	<table>
		<thead>
			<tr>
				{#each columns as c, i (i)}
					<th class:num={c.num}>{c.label}</th>
				{/each}
			</tr>
		</thead>
		<tbody>
			{#each items as item (key(item))}
				<tr
					class:selected={selectable && selected === key(item)}
					onclick={selectable
						? () => (selected = key(item))
						: undefined}
					ondblclick={ondblclick && (() => ondblclick(item))}
				>
					{@render row(item)}
				</tr>
			{:else}
				{#if empty !== undefined}
					<tr>
						<td colspan={columns.length} class="empty">{empty}</td>
					</tr>
				{/if}
			{/each}
		</tbody>
	</table>
</div>

<style>
	.fill {
		flex: 1;
	}
</style>
