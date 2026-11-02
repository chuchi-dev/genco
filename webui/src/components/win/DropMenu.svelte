<script lang="ts">
	import type { Snippet } from 'svelte';
	import MenuList from './MenuList.svelte';
	import type { MenuEntry } from './Menu.svelte';

	// a button that drops a menu down (or up): a click elsewhere or Escape
	// closes it
	let {
		entries,
		align = 'left',
		open = $bindable(false),
		button,
		children,
	}: {
		entries: MenuEntry[];
		/** where it opens: below from the left or right edge, or above */
		align?: 'left' | 'right' | 'up';
		open?: boolean;
		/** the toggle, it calls `toggle` on a click */
		button: Snippet<[{ open: boolean; toggle: (e: MouseEvent) => void }]>;
		/** shown next to the entries, e.g. the Start menu's brand */
		children?: Snippet;
	} = $props();

	function toggle(e: MouseEvent) {
		// the window's click would close it again
		e.stopPropagation();
		open = !open;
	}

	// only an open one closes, menus sharing a state (a menu bar) stay
	function close() {
		if (open) open = false;
	}
</script>

<svelte:window onclick={close} onkeydown={e => e.key === 'Escape' && close()} />

<div class="drop-menu">
	{@render button({ open, toggle })}
	{#if open}
		<div class="drop {align}">
			{@render children?.()}
			<MenuList {entries} onpick={close} />
		</div>
	{/if}
</div>

<style lang="scss">
	.drop-menu {
		position: relative;
	}

	.drop {
		position: absolute;
		z-index: 50;
		white-space: nowrap;

		&.left {
			top: 100%;
			left: 0;
		}

		&.right {
			top: 100%;
			right: 0;
		}

		&.up {
			bottom: 100%;
			left: 0;
		}
	}
</style>
