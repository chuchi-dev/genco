<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		tabs,
		selected,
		onselect,
		children,
	}: {
		tabs: { id: string; label: string }[];
		selected: string;
		onselect: (id: string) => void;
		children: Snippet;
	} = $props();
</script>

<div class="tabs">
	<div class="list" role="tablist">
		{#each tabs as tab (tab.id)}
			<button
				role="tab"
				class:active={tab.id === selected}
				aria-selected={tab.id === selected}
				onclick={() => onselect(tab.id)}
			>
				{tab.label}
			</button>
		{/each}
	</div>
	<div class="panel" role="tabpanel">
		{@render children()}
	</div>
</div>

<style lang="scss">
	@use 'styles/win' as *;

	.tabs {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
	}

	.list {
		display: flex;
		// a narrow window gets a second row, like Windows' tabs
		flex-wrap: wrap;
		padding-left: 2px;
		position: relative;
		z-index: 1;
	}

	button {
		background: $face;
		border: none;
		font: inherit;
		padding: 3px 10px 2px;
		margin-top: 2px;
		box-shadow:
			inset -1px 0 $dark,
			inset 1px 1px $white,
			inset -2px 0 $shadow,
			inset 2px 2px $light;

		&.active {
			margin: 0 -2px -1px;
			padding: 4px 12px 4px;
			z-index: 1;
		}

		&:focus-visible {
			outline: 1px dotted #000;
			outline-offset: -4px;
		}
	}

	.panel {
		@include raised;
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
		padding: 10px;
		overflow: auto;
	}
</style>
