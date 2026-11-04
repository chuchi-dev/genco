<script lang="ts" module>
	import type { IconName } from './Icon.svelte';

	export type TreeItem = {
		key: string;
		label: string;
		icon: IconName;
		/** the shortcut arrow over the icon */
		link?: boolean;
		/** shown on hover */
		title?: string;
		/** a dot after the label: running, failed or stopped */
		state?: 'ok' | 'bad' | 'off';
		children?: TreeItem[];
	};
</script>

<script lang="ts">
	import Icon from './Icon.svelte';
	import Tree from './Tree.svelte';

	let {
		items,
		selected,
		onselect,
		oncontext,
		closed = $bindable(new Set<string>()),
	}: {
		items: TreeItem[];
		selected: string | null;
		onselect: (key: string) => void;
		/// a right click on an item, the browser's own menu stays closed
		oncontext?: (key: string, e: MouseEvent) => void;
		/// keys of the items whose children are hidden
		closed?: Set<string>;
	} = $props();

	function toggle(key: string) {
		const next = new Set(closed);
		if (next.has(key)) next.delete(key);
		else next.add(key);
		closed = next;
	}
</script>

<ul>
	{#each items as item (item.key)}
		{@const open = !closed.has(item.key)}
		<li>
			<div class="row">
				{#if item.children?.length}
					<button
						class="toggle"
						onclick={() => toggle(item.key)}
						aria-label={open ? 'Collapse' : 'Expand'}
					>
						{open ? '−' : '+'}
					</button>
				{:else}
					<span class="leaf"></span>
				{/if}
				<button
					class="label"
					class:selected={selected === item.key}
					title={item.title}
					onclick={() => onselect(item.key)}
					ondblclick={() => toggle(item.key)}
					oncontextmenu={e => {
						if (!oncontext) return;
						e.preventDefault();
						// the window's handler would close the new menu
						e.stopPropagation();
						oncontext(item.key, e);
					}}
				>
					<Icon name={item.icon} link={item.link} />
					<span>{item.label}</span>
					{#if item.state}
						<i class="dot {item.state}"></i>
					{/if}
				</button>
			</div>
			{#if item.children?.length && open}
				<Tree
					items={item.children}
					{selected}
					{onselect}
					{oncontext}
					bind:closed
				/>
			{/if}
		</li>
	{/each}
</ul>

<style lang="scss">
	@use 'styles/win' as *;

	ul {
		list-style: none;
		margin: 0;
		padding: 0;

		:global(ul) {
			padding-left: 16px;
		}
	}

	.row {
		display: flex;
		align-items: center;
		gap: 3px;
		height: 18px;
	}

	.toggle,
	.leaf {
		width: 9px;
		height: 9px;
		flex-shrink: 0;
	}

	.toggle {
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 0;
		border: 1px solid $shadow;
		background: $white;
		font: 9px/1 $mono;
	}

	.label {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 0 2px 0 0;
		border: none;
		background: none;
		font: inherit;
		white-space: nowrap;

		span {
			padding: 1px 2px;
		}

		&.selected span {
			background: $select;
			color: $white;
			outline: 1px dotted #ff0;
		}
	}

	.dot {
		width: 6px;
		height: 6px;
		flex-shrink: 0;
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
