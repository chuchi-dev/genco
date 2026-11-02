<script lang="ts">
	import Dialog from '@/components/win/Dialog.svelte';
	import Button from '@/components/win/Button.svelte';
	import { go } from '@/lib/router.svelte';
</script>

<Dialog title="Not Found" onclose={() => go('/')}>
	<p>{location.pathname} doesn't exist.</p>
	<div class="row-buttons center">
		<Button primary onclick={() => go('/')}>OK</Button>
	</div>
</Dialog>

<style>
	p {
		margin: 4px 0 16px;
	}
</style>
