<script lang="ts">
	import type { MenuEntry } from './Menu.svelte';

	let { entries, onpick }: { entries: MenuEntry[]; onpick: () => void } =
		$props();
</script>

<ul>
	{#each entries as entry, i (i)}
		{#if 'separator' in entry}
			<li class="sep"></li>
		{:else}
			<li>
				<button
					disabled={entry.disabled}
					onclick={e => {
						e.stopPropagation();
						onpick();
						entry.onclick();
					}}
				>
					{entry.label}
				</button>
			</li>
		{/if}
	{/each}
</ul>

<style lang="scss">
	@use 'styles/win' as *;

	ul {
		@include raised;
		list-style: none;
		margin: 0;
		padding: 3px;
		min-width: 170px;
		box-shadow:
			inset -1px -1px $dark,
			inset 1px 1px $light,
			inset -2px -2px $shadow,
			inset 2px 2px $white,
			2px 2px 0 rgba(0, 0, 0, 0.4);
	}

	button {
		display: block;
		width: 100%;
		text-align: left;
		border: none;
		background: none;
		font: inherit;
		padding: 3px 20px;
		white-space: nowrap;

		@include hover {
			background: $select;
			color: $white;
		}

		&:disabled {
			color: $shadow;
			background: none;
		}
	}

	.sep {
		margin: 3px 1px;
		height: 2px;
		@include groove;
	}
</style>
