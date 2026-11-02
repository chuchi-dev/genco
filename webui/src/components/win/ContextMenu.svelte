<script lang="ts">
	import MenuList from './MenuList.svelte';
	import type { MenuEntry } from './Menu.svelte';

	// a menu at the pointer, opened by a right click; a click elsewhere,
	// Escape or leaving the page closes it
	let {
		x,
		y,
		entries,
		onclose,
	}: {
		x: number;
		y: number;
		entries: MenuEntry[];
		onclose: () => void;
	} = $props();

	let el: HTMLDivElement;
	// svelte-ignore state_referenced_locally
	let pos = $state({ x, y });

	// it stays inside the window, near a corner it opens to the other side
	$effect(() => {
		const r = el.getBoundingClientRect();
		pos = {
			x: x + r.width > innerWidth - 4 ? Math.max(4, x - r.width) : x,
			y: y + r.height > innerHeight - 4 ? Math.max(4, y - r.height) : y,
		};
	});
</script>

<svelte:window
	onclick={onclose}
	oncontextmenu={onclose}
	onblur={onclose}
	onresize={onclose}
	onkeydown={e => e.key === 'Escape' && onclose()}
/>

<div class="context" bind:this={el} style="left: {pos.x}px; top: {pos.y}px">
	<MenuList {entries} onpick={onclose} />
</div>

<style lang="scss">
	.context {
		position: fixed;
		z-index: 100;
		white-space: nowrap;
	}
</style>
