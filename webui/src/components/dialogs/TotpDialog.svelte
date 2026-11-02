<script lang="ts">
	import { post } from '@/lib/api';
	import { session } from '@/lib/session.svelte';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	// turns two-factor login on or off for the logged in admin
	let { onclose }: { onclose: () => void } = $props();

	const enabled = $derived(session.user?.totp ?? false);
	let setup = $state<{ secret: string; qr: string } | null>(null);
	let code = $state('');
	let password = $state('');
	let error = $state<string | null>(null);
	let done = $state<string | null>(null);

	async function start() {
		error = null;
		try {
			setup = await post('/users/totp/setup');
		} catch (e: any) {
			error = e.message;
		}
	}

	async function enable() {
		await post('/users/totp/enable', {
			secret: setup!.secret,
			code,
			password,
		});
		session.user = { ...session.user!, totp: true };
		done = 'Two-factor login is on. The next login asks for a code.';
	}

	async function disable() {
		await post('/users/totp/disable', { code, password });
		session.user = { ...session.user!, totp: false };
		done = 'Two-factor login is off.';
	}
</script>

<Dialog title="Two-Factor Login" icon="key" {onclose} width={420}>
	{#if done}
		<p>{done}</p>
		<div class="row-buttons end">
			<Button primary onclick={onclose}>OK</Button>
		</div>
	{:else if enabled}
		<p>
			Two-factor login is on. To turn it off, type your password and a
			current code.
		</p>
		<Form onsubmit={disable} oncancel={onclose} ok="Turn Off">
			<label for="t-pw">Password:</label>
			<!-- svelte-ignore a11y_autofocus -->
			<input
				id="t-pw"
				autofocus
				type="password"
				autocomplete="current-password"
				bind:value={password}
			/>
			<label for="t-code">Code:</label>
			<input id="t-code" inputmode="numeric" bind:value={code} />
		</Form>
	{:else if setup}
		<p>
			Scan the code with an authenticator app, then type the code it
			shows.
		</p>
		<div class="qr">
			<img src="data:image/png;base64,{setup.qr}" alt="QR code" />
			<code class="selectable">{setup.secret}</code>
		</div>
		<Form onsubmit={enable} oncancel={onclose} ok="Turn On">
			<label for="t-pw">Password:</label>
			<!-- svelte-ignore a11y_autofocus -->
			<input
				id="t-pw"
				autofocus
				type="password"
				autocomplete="current-password"
				bind:value={password}
			/>
			<label for="t-code">Code:</label>
			<input id="t-code" inputmode="numeric" bind:value={code} />
		</Form>
	{:else}
		<p>
			With two-factor login, logging in also needs a code of an
			authenticator app on your phone.
		</p>
		{#if error}<p class="error">{error}</p>{/if}
		<div class="row-buttons end">
			<Button primary onclick={start}>Set Up…</Button>
			<Button onclick={onclose}>Cancel</Button>
		</div>
	{/if}
</Dialog>

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 0 0 10px;
	}

	.qr {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 6px;
		margin-bottom: 12px;

		img {
			width: 180px;
			height: 180px;
			image-rendering: pixelated;
			background: white;
		}

		code {
			font-size: 11px;
		}
	}
</style>
