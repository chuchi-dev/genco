<script lang="ts">
	import { get } from '@/lib/api';
	import Button from '@/components/win/Button.svelte';

	// the SMTP login of a group's sites: mail sent with it is DKIM signed,
	// mail() over localhost:25 without a login is not
	let { groupId }: { groupId: number } = $props();

	type Login = {
		host: string;
		port: number;
		user: string;
		password: string;
		domains: string[];
	};

	let login = $state<Login | null | undefined>(undefined);
	let error = $state<string | null>(null);

	async function show() {
		try {
			login = await get(`/groups/${groupId}/smtp`);
		} catch (e: any) {
			error = e.message;
		}
	}
</script>

<h4>Mail from the sites</h4>
<p class="hint">
	Sites and apps that send with this SMTP login get their mail DKIM signed, as
	any address of the group's mail domains. Apps have it as
	<code>GENCO_SMTP_*</code>.
</p>
{#if login}
	<dl class="props">
		<dt>Server:</dt>
		<dd><code>{login.host}:{login.port}</code> (STARTTLS)</dd>
		<dt>User:</dt>
		<dd><code class="selectable">{login.user}</code></dd>
		<dt>Password:</dt>
		<dd><code class="selectable">{login.password}</code></dd>
		<dt>Senders:</dt>
		<dd>any address at {login.domains.join(', ')}</dd>
	</dl>
{:else if login === null}
	<p class="hint">It comes with the next Server → Apply Changes.</p>
{:else}
	<Button onclick={show}>Show Login</Button>
{/if}
{#if error}<p class="error">{error}</p>{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.hint {
		margin: 0 0 6px;
	}
</style>
