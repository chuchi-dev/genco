<script lang="ts">
	import Dialog from '@/components/win/Dialog.svelte';
	import Button from '@/components/win/Button.svelte';

	let { error }: { error: any } = $props();
</script>

<Dialog title="Error" onclose={() => location.reload()}>
	<p>{error?.message ?? String(error)}</p>
	<div class="row-buttons center">
		<Button primary onclick={() => location.reload()}>Try again</Button>
	</div>
</Dialog>

<style>
	p {
		margin: 4px 0 16px;
		user-select: text;
	}
</style>
