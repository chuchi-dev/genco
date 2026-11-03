<script lang="ts" module>
	import { get } from '@/lib/api';

	export async function loadProps({ token }: { token: string }) {
		return { email: (await get(`/users/email/${token}`)) as string };
	}
</script>

<script lang="ts">
	import { post, type User } from '@/lib/api';
	import { go } from '@/lib/router.svelte';
	import { session } from '@/lib/session.svelte';
	import Button from '@/components/win/Button.svelte';
	import Dialog from '@/components/win/Dialog.svelte';
	import Icon from '@/components/win/Icon.svelte';

	// the link of the mail to a new email: the admin confirms it
	let { token, email }: { token: string; email: string } = $props();

	// the link's token doesn't stay in the address bar and the history
	history.replaceState(history.state, '', '/email');

	let busy = $state(false);
	let done = $state(false);
	let error = $state<string | null>(null);

	async function confirmIt() {
		busy = true;
		try {
			session.user = (await post(`/users/email/${token}`)) as User;
			done = true;
		} catch (e: any) {
			error = e.message;
		}
		busy = false;
	}
</script>

<Dialog title="Confirm Email" icon="key" onclose={() => go('/')} width={420}>
	<div class="msg">
		<Icon name="key" size={32} />
		<p>
			{#if done}
				Done, log in with {email} from now on.
			{:else}
				Use {email} as the email of {session.user?.name}? It's how you
				log in and where genco's mails go.
			{/if}
		</p>
	</div>
	{#if error}<p class="error">{error}</p>{/if}
	<div class="row-buttons end">
		{#if done}
			<Button primary onclick={() => go('/')}>Close</Button>
		{:else}
			<Button primary onclick={confirmIt} disabled={busy}>Confirm</Button>
			<Button onclick={() => go('/')}>Cancel</Button>
		{/if}
	</div>
</Dialog>

<style lang="scss">
	.msg {
		display: flex;
		gap: 12px;
	}

	p {
		margin: 0 0 8px;
	}
</style>
