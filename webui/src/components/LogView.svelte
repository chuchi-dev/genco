<script lang="ts">
	import { newest, request } from '@/lib/api';

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
	let error = $state<string | null>(null);
	// an older response doesn't overwrite a newer one
	const latest = newest();

	function load(from: string) {
		latest(
			request('GET', from, null, true),
			(t: string) => {
				text = t;
				error = null;
			},
			e => (error = e.message),
		);
	}

	// once when the log or its state changes, then while it runs
	$effect(() => {
		const from = url;
		load(from);
		if (!live) return;
		const iv = setInterval(() => load(from), 2000);
		return () => clearInterval(iv);
	});
</script>

<pre class="log selectable" style:height="{height}px">{error ??
		(text || '…')}</pre>

<style lang="scss">
	@use 'styles/win' as *;

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
