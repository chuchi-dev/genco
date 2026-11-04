<script lang="ts">
	import { newest, request } from '@/lib/api';
	import { Follow } from '@/lib/follow.svelte';
	import Button from './win/Button.svelte';

	// the log of a deploy or backup, every two seconds while it runs
	let {
		url,
		live,
		height = 220,
	}: {
		/** where the log is, e.g. /deploys/3/log */
		url: string;
		/** it still runs, the log grows */
		live: boolean;
		height?: number;
	} = $props();

	let text = $state('');
	let loaded = $state(false);
	let error = $state<string | null>(null);
	// an older response doesn't overwrite a newer one
	const latest = newest();

	function load(from: string) {
		latest(
			request('GET', from, null, true),
			(t: string) => {
				text = t;
				error = null;
				loaded = true;
			},
			e => (error = e.message),
		);
	}

	// stays at the end while it grows, unless the reader scrolled up
	const follow = new Follow();
	// another log starts at its end
	$effect(() => {
		void url;
		follow.down();
	});

	// once when the log or its state changes, then while it runs
	$effect(() => {
		const from = url;
		load(from);
		if (!live) return;
		const iv = setInterval(() => load(from), 2000);
		return () => clearInterval(iv);
	});
</script>

<!-- old logs are removed after a while, their runs stay -->
<div class="log-box">
	<pre
		class="log selectable"
		style:height="{height}px"
		{@attach follow.attach}>{error ??
			(text ||
				(loaded && !live
					? 'No log, removed after a while.'
					: '…'))}</pre>
	{#if follow.unseen}
		<div class="unseen">
			<Button onclick={() => follow.down()}>↓ New output</Button>
		</div>
	{/if}
</div>

<style lang="scss">
	@use 'styles/win' as *;

	.log-box {
		position: relative;
	}

	.unseen {
		position: absolute;
		right: 20px;
		bottom: 6px;
	}

	.log {
		margin: 0;
		padding: 6px;
		background: $terminal-bg;
		color: $terminal-fg;
		overflow: auto;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
</style>
