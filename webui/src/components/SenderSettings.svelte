<script lang="ts">
	import { get, type SendOnly, type SenderSettings } from '@/lib/api';
	import Button from '@/components/win/Button.svelte';

	// what an app needs to send as a send-only address; the password and
	// the URLs with it only once asked for, the activity log records who
	let { sender, host }: { sender: SendOnly; host: string | null } = $props();

	let settings = $state<SenderSettings | null>(null);
	let error = $state<string | null>(null);

	async function show() {
		error = null;
		try {
			settings = await get(`/mail-senders/${sender.id}`);
		} catch (e: any) {
			error = e.message;
		}
	}
</script>

<fieldset>
	<legend>App settings of {sender.address}</legend>
	<dl class="props">
		<dt>Host:</dt>
		<dd><code class="selectable">{settings?.host ?? host ?? '…'}</code></dd>
		<dt>Port:</dt>
		<dd><code class="selectable">587</code> (STARTTLS)</dd>
		<dt>User:</dt>
		<dd><code class="selectable">{sender.address}</code></dd>
		<dt>Password:</dt>
		<dd>
			{#if settings}
				<code class="selectable">{settings.password}</code>
			{:else}
				<Button onclick={show}>Show</Button>
			{/if}
		</dd>
		<dt>From:</dt>
		<dd><code class="selectable">{sender.address}</code></dd>
	</dl>
	{#if settings}
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
	{/if}
	<p class="hint">
		Craft: Settings → Email, transport SMTP, the host, port 587, encryption
		tls, the user and password above, the From address as the system email.
	</p>
	{#if error}<p class="error">{error}</p>{/if}
</fieldset>

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 6px 0 4px;
	}
</style>
