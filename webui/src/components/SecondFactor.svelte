<script lang="ts">
	import { session } from '@/lib/session.svelte';

	// what confirms in Approvals and on a mail-off link, rows of a Form: the admin's password and,
	// with 2FA, the code of the authenticator app
	let {
		password = $bindable(''),
		code = $bindable(''),
		autofocus = false,
	}: { password?: string; code?: string; autofocus?: boolean } = $props();
</script>

<label for="sf-password">Your password:</label>
<!-- svelte-ignore a11y_autofocus -->
<input
	id="sf-password"
	type="password"
	autocomplete="current-password"
	{autofocus}
	bind:value={password}
/>
{#if session.user?.totp}
	<label for="sf-code">Two-factor code:</label>
	<input
		id="sf-code"
		inputmode="numeric"
		autocomplete="one-time-code"
		bind:value={code}
	/>
{/if}
