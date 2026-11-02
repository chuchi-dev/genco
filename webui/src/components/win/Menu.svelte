<script lang="ts" module>
	export type MenuEntry =
		| { label: string; onclick: () => void; disabled?: boolean }
		| { separator: true };
	export type MenuDef = { label: string; entries: MenuEntry[] };
</script>

<script lang="ts">
	import DropMenu from './DropMenu.svelte';

	// the menu bar of a window: File, View, ...
	let { menus }: { menus: MenuDef[] } = $props();

	// one open at a time, the pointer moves it to the next one
	let open = $state<number | null>(null);
</script>

{#each menus as menu, i (menu.label)}
	<DropMenu
		entries={menu.entries}
		bind:open={() => open === i, v => (open = v ? i : null)}
	>
		{#snippet button({ open: shown, toggle })}
			<button
				class:open={shown}
				onclick={toggle}
				onmouseenter={() => {
					if (open !== null) open = i;
				}}
			>
				{menu.label}
			</button>
		{/snippet}
	</DropMenu>
{/each}

<style lang="scss">
	@use 'styles/win' as *;

	button {
		border: none;
		background: none;
		font: inherit;
		padding: 2px 7px;

		&.open {
			box-shadow:
				inset -1px -1px $white,
				inset 1px 1px $shadow;
		}
	}
</style>
