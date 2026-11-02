<script lang="ts">
	import { onDestroy } from 'svelte';
	import { poll } from '@/lib/poll';
	import { formatTime, get, post } from '@/lib/api';
	import Button from '../win/Button.svelte';

	// genco's own version: the newest release, installing it, the night's
	// patch releases
	type Status = {
		current: string;
		latest: string | null;
		checkedAt: number | null;
		error: string | null;
		auto: boolean;
		mode: 'release' | 'testing';
		running: boolean;
		last: {
			from: string;
			to: string;
			ok: boolean;
			at: number;
			message: string;
		} | null;
	};

	let status = $state<Status | null>(null);
	let error = $state<string | null>(null);
	let busy = $state(false);
	let log = $state('');

	function load() {
		return get('/update')
			.then((s: Status) => {
				status = s;
				error = null;
			})
			.catch(e => (error = e.message));
	}
	// while an update runs genco restarts: keep asking, errors included,
	// one request after the other, until it ran and its log is in
	let stop: (() => void) | null = null;
	// an update that ended before this one started doesn't count
	let since = 0;
	function watch() {
		stop ??= poll(async () => {
			await load();
			log = await get('/update/log').catch(() => log);
			const ended = !status?.running && (status?.last?.at ?? 0) >= since;
			if (ended) stop = null;
			return !ended;
		}, 3000);
	}
	onDestroy(() => stop?.());

	// one runs already when the dialog opens
	load().then(() => {
		if (status?.running) watch();
	});

	function newer(a: string, b: string): boolean {
		const p = (v: string) => v.split('.').map(Number);
		const [x, y] = [p(a), p(b)];
		for (let i = 0; i < 3; i++) {
			if (x[i] !== y[i]) return x[i] > y[i];
		}
		return false;
	}
	const available = $derived(
		status?.latest && newer(status.latest, status.current)
			? status.latest
			: null,
	);

	async function act(fn: () => Promise<unknown>) {
		busy = true;
		error = null;
		try {
			await fn();
		} catch (e: any) {
			error = e.message;
		} finally {
			busy = false;
		}
	}

	const check = () =>
		act(async () => (status = await post('/update/check', {})));

	const install = () =>
		act(async () => {
			await post('/update/install', {});
			log = 'Starting…';
			since = Date.now() - 5000;
			watch();
		});

	const setAuto = (auto: boolean) =>
		act(async () => {
			await post('/update/auto', { auto });
			await load();
		});
</script>

{#if status}
	<dl class="props">
		<dt>Installed:</dt>
		<dd>genco {status.current}</dd>
		<dt>Newest:</dt>
		<dd>
			{#if status.error}
				<span class="error">{status.error}</span>
			{:else if status.latest}
				{status.latest}
				{#if available}
					<a
						href="https://github.com/chuchi-dev/genco/releases/tag/v{available}"
						target="_blank"
						rel="noreferrer noopener">(release notes)</a
					>
				{:else}
					(up to date)
				{/if}
			{:else}
				not checked yet
			{/if}
		</dd>
		<dt>Mode:</dt>
		<dd>
			{#if status.mode === 'testing'}
				Testing: never updates itself, builds come with
				<code>genco update --file</code>
			{:else}
				Releases: signed versions from GitHub
			{/if}
			<span class="hint"
				>(<code>genco update --mode</code> on the terminal)</span
			>
		</dd>
		<dt>Checked:</dt>
		<dd>{status.checkedAt ? formatTime(status.checkedAt) : 'never'}</dd>
		{#if status.last}
			<dt>Last update:</dt>
			<dd class:error={!status.last.ok}>
				{formatTime(status.last.at)}: {status.last.message}
			</dd>
		{/if}
	</dl>

	<div class="buttons">
		<Button onclick={check} disabled={busy || status.running}>
			Check Now
		</Button>
		<Button
			primary
			onclick={install}
			disabled={busy || status.running || !available}
		>
			{status.running
				? 'Updating…'
				: available
					? `Update to ${available}`
					: 'Update'}
		</Button>
	</div>

	<label class="check" class:off={status.mode === 'testing'}>
		<input
			type="checkbox"
			checked={status.auto && status.mode !== 'testing'}
			disabled={busy || status.mode === 'testing'}
			onchange={e => setAuto(e.currentTarget.checked)}
		/>
		Install patch releases (like {status.current.replace(/\.\d+$/, '.x')})
		at night by themselves
	</label>

	<p class="hint">
		An update checks the release's signature, saves the binary and the
		database, restarts genco and goes back to this version if the new one
		doesn't come up. Minor and major releases come in the weekly update
		round's report; on the terminal it is <code>genco update --yes</code>.
	</p>

	{#if log}
		<pre class="log selectable">{log}</pre>
	{/if}
{/if}
{#if error}<p class="error">{error}</p>{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.props {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: 4px 10px;
		margin: 0 0 10px;

		dd {
			margin: 0;
		}
	}

	.buttons {
		display: flex;
		gap: 6px;
		margin-bottom: 10px;
	}

	.off {
		color: $hint;
	}

	.check {
		margin-bottom: 10px;
	}

	.hint {
		margin: 0 0 8px;
	}

	.log {
		@include sunken;
		background: $white;
		max-height: 160px;
		overflow: auto;
		margin: 0;
		padding: 4px;
		font-size: 11px;
	}
</style>
