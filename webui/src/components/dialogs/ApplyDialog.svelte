<script lang="ts">
	import { post, type ApplyReport } from '@/lib/api';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';

	// writes the config of all groups and shows what changed
	let { onclose }: { onclose: () => void } = $props();

	let report = $state<ApplyReport | null>(null);
	let error = $state<string | null>(null);

	post('/apply')
		.then(r => (report = r))
		.catch(e => (error = e.message));
</script>

<Dialog title="Apply Changes" resizable icon="gear" {onclose} width={560}>
	{#if error}
		<p class="error">{error}</p>
	{:else if !report}
		<p>
			Writing the config, starting services and getting certificates… This
			can take a minute.
		</p>
		<div class="progress"><span></span></div>
	{:else}
		<p>{report.ok ? 'Everything is applied.' : 'Applied with errors:'}</p>
		<pre class="log selectable">{report.log.join('\n')}</pre>
	{/if}
	<div class="row-buttons end">
		<Button primary onclick={onclose} disabled={!report && !error}>
			Close
		</Button>
	</div>
</Dialog>

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 0 0 10px;
	}

	.log {
		@include sunken;
		margin: 0;
		padding: 6px;
		flex: 1;
		min-height: 120px;
		overflow: auto;
		white-space: pre-wrap;
	}

	.progress {
		@include sunken;
		height: 20px;
		padding: 3px;
		overflow: hidden;

		span {
			display: block;
			height: 100%;
			width: 40%;
			background: repeating-linear-gradient(
				90deg,
				$title 0 10px,
				transparent 10px 12px
			);
			animation: slide 1.5s linear infinite;
		}
	}

	@keyframes slide {
		from {
			transform: translateX(-100%);
		}
		to {
			transform: translateX(250%);
		}
	}
</style>
