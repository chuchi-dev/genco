<script lang="ts">
	import Dialog from '@/components/win/Dialog.svelte';
	import Button from '@/components/win/Button.svelte';
	import { GoneError, go } from '@/lib/router.svelte';

	let { error }: { error: any } = $props();

	// trying again can't bring back what is gone
	// svelte-ignore state_referenced_locally
	const gone = error instanceof GoneError;
	const close = () => (gone ? go('/', true) : location.reload());
</script>

<Dialog title={gone ? 'Gone' : 'Error'} onclose={close}>
	<p>{error?.message ?? String(error)}</p>
	<div class="row-buttons center">
		<Button primary onclick={close}>
			{gone ? 'Back to the Server' : 'Try again'}
		</Button>
	</div>
</Dialog>

<style>
	p {
		margin: 4px 0 16px;
		user-select: text;
	}
</style>
