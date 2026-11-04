<script lang="ts">
	import { get, type SendOnly, type SendOnlySettings } from '@/lib/api';
	import Button from '@/components/win/Button.svelte';

	// what an app needs to send as a send-only address, with the password
	// and the URLs with it only once asked for, the activity log records
	// who
	let { sendOnly }: { sendOnly: SendOnly } = $props();

	let settings = $state<SendOnlySettings | null>(null);
	let error = $state<string | null>(null);

	async function show() {
		error = null;
		try {
			settings = await get(`/send-only/${sendOnly.id}`);
		} catch (e: any) {
			error = e.message;
		}
	}
</script>

<fieldset>
	<legend>App settings of {sendOnly.address}</legend>
	{#if settings}
		<dl class="props">
			<dt>Host:</dt>
			<dd><code class="selectable">{settings.host}</code></dd>
			<dt>Port:</dt>
			<dd><code class="selectable">{settings.port}</code> (STARTTLS)</dd>
			<dt>User:</dt>
			<dd><code class="selectable">{settings.user}</code></dd>
			<dt>Password:</dt>
			<dd><code class="selectable">{settings.password}</code></dd>
			<dt>From:</dt>
			<dd><code class="selectable">{settings.from}</code></dd>
		</dl>
		<p>For a zipp app:</p>
		<pre class="code selectable">ZIPP_SMTP_URL={settings.zippSmtpUrl}</pre>
		<p>Other apps that take a URL:</p>
		<pre class="code selectable">SMTP_URL={settings.smtpUrl}</pre>
		{#if !settings.ready}
			<p class="error">
				The mail host has no certificate yet, sending works once Server
				→ Apply Changes got it.
			</p>
		{/if}
		<p class="hint">
			Craft: Settings → Email, transport SMTP, host {settings.host}, port
			{settings.port}, encryption tls, the user and password above, {settings.from}
			as the system email.
		</p>
	{:else}
		<p>Host, port, login and password for the app:</p>
		<Button onclick={show}>Show</Button>
	{/if}
	{#if error}<p class="error">{error}</p>{/if}
</fieldset>

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 6px 0 4px;
	}
</style>
