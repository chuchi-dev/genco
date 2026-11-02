<script lang="ts">
	import { view } from '@/lib/router.svelte';

	/** The page of the router in its frame, see lib/router.svelte.ts. */
	const page = $derived(view.page!);
</script>

<!-- the frame stays from page to page, the page is new -->
{#if page.layout}
	<page.layout {...page.props}>
		{#key page.key}
			<page.component {...page.props} />
		{/key}
	</page.layout>
{:else}
	{#key page.key}
		<page.component {...page.props} />
	{/key}
{/if}
