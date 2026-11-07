<script lang="ts">
	import { session } from '@/lib/session.svelte';

	// the step-up of Approvals, a mail-off link and the account, rows of a
	// Form: the code of the authenticator app with 2FA, else the password
	let {
		password = $bindable(''),
		code = $bindable(''),
		autofocus = false,
	}: { password?: string; code?: string; autofocus?: boolean } = $props();
</script>

{#if session.user?.totp}
	<label for="sf-code">Two-factor code:</label>
	<!-- svelte-ignore a11y_autofocus -->
	<input
		id="sf-code"
		inputmode="numeric"
		autocomplete="one-time-code"
		{autofocus}
		bind:value={code}
	/>
{:else}
	<label for="sf-password">Your password:</label>
	<!-- svelte-ignore a11y_autofocus -->
	<input
		id="sf-password"
		type="password"
		autocomplete="current-password"
		{autofocus}
		bind:value={password}
	/>
{/if}
