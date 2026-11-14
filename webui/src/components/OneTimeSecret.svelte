<script lang="ts">
	import type { Snippet } from 'svelte';
	import Button from './win/Button.svelte';
	import Dialog from './win/Dialog.svelte';

	// a token (or a command with one) that the server shows this once, in
	// its own window over the one that made it: a click beside it doesn't
	// close it, only OK, Escape or the title bar's close
	let {
		title,
		value,
		label = 'Copy it now, it is shown only once:',
		onclose,
		children,
	}: {
		title: string;
		value: string;
		label?: string;
		onclose: () => void;
		/** what to do with it, below the value */
		children?: Snippet;
	} = $props();

	let copied = $state(false);
	let error = $state<string | null>(null);

	async function copy() {
		try {
			await navigator.clipboard.writeText(value);
			copied = true;
			setTimeout(() => (copied = false), 2000);
		} catch (e: any) {
			error = e.message;
		}
	}
</script>

<Dialog {title} icon="key" {onclose} keep={() => true} width={520}>
	<p><b>{label}</b></p>
	<pre class="code selectable">{value}</pre>
	{#if error}<p class="error">{error}</p>{/if}
	{@render children?.()}
	<div class="row-buttons center">
		<Button onclick={copy}>{copied ? 'Copied' : 'Copy'}</Button>
		<Button primary onclick={onclose}>OK</Button>
	</div>
</Dialog>

<style>
	p {
		margin: 0 0 8px;
	}

	pre {
		margin: 0 0 10px;
	}
</style>
