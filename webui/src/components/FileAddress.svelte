<script lang="ts">
	// the address bar of the file manager: the group's folder and each
	// folder below it, a click opens it
	let {
		root,
		parts,
		href,
	}: {
		/** the group's folder on the server */
		root: string;
		/** the folders from the root to the open one */
		parts: string[];
		/** the page of a folder, a path relative to the root */
		href: (path: string) => string;
	} = $props();
</script>

<div class="address">
	<span>Address</span>
	<div class="path">
		<a href={href('')}>{root}</a>
		{#each parts as part, i (i)}
			<span>/</span>
			<a href={href(parts.slice(0, i + 1).join('/'))}>{part}</a>
		{/each}
	</div>
</div>

<style lang="scss">
	@use 'styles/win' as *;

	.address {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-bottom: 6px;

		.path {
			@include sunken;
			flex: 1;
			display: flex;
			flex-wrap: wrap;
			align-items: center;
			padding: 3px 4px;
			min-height: 22px;
			font-family: $mono;
		}

		a {
			color: inherit;
			text-decoration: none;
			padding: 0 1px;

			@include hover {
				text-decoration: underline;
			}
		}
	}
</style>
