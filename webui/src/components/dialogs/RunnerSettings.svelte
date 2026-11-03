<script lang="ts">
	import OneTimeSecret from '../OneTimeSecret.svelte';
	import { onDestroy } from 'svelte';
	import { formatTime, get, post } from '@/lib/api';
	import { message, confirmDelete } from '@/lib/dialogs.svelte';
	import Button from '../win/Button.svelte';
	import Form from '../win/Form.svelte';
	import ListView from '../win/ListView.svelte';

	// the build runners of this server: machines that build the steps with
	// on = "runner" (genco runner, connects out to this server)
	type Runner = {
		id: number;
		name: string;
		lastSeen: number | null;
		online: boolean;
		job: number | null;
		info: {
			host?: string;
			engine?: string;
			cpus?: number;
			version?: string;
		};
	};
	type Target = {
		os: string;
		version: string;
		codename: string;
		glibc: string;
		arch: string;
	};

	let runners = $state<Runner[]>([]);
	let target = $state<Target | null>(null);
	// genco's own, a runner should run the same
	let version = $state('');
	let selected = $state<string | null>(null);
	let adding = $state(false);
	let name = $state('');
	let created = $state<string | null>(null);
	let error = $state<string | null>(null);

	async function load() {
		try {
			const r = await get('/runners');
			runners = r.runners;
			target = r.target;
			version = r.version;
			error = null;
		} catch (e: any) {
			error = e.message;
		}
	}
	load();
	const outdated = $derived(
		runners.filter(r => r.info.version && r.info.version !== version),
	);
	// online changes while the dialog is open
	const poll = setInterval(load, 10000);
	onDestroy(() => clearInterval(poll));

	async function add() {
		const password = await message.password(
			'New Runner',
			'create a runner token',
		);
		if (password === null) return;
		const token = await post('/runners', { name, password });
		created = `genco runner register ${location.origin} ${token}`;
		adding = false;
		name = '';
		await load();
	}

	async function remove() {
		const removed =
			selected &&
			(await confirmDelete({
				title: 'Remove Runner',
				text: `Remove the runner ${selected}? Its token stops working, a build it runs fails.`,
				path: '/runners/' + encodeURIComponent(selected),
			}));
		if (!removed) return;
		selected = null;
		await load();
	}

	function runnerState(r: Runner): string {
		if (r.job) return `building (job ${r.job})`;
		return r.online ? 'online' : 'offline';
	}
</script>

{#if adding}
	<Form onsubmit={add} oncancel={() => (adding = false)} ok="Create">
		<label for="r-name">Name:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			id="r-name"
			autofocus
			placeholder="e.g. performance"
			bind:value={name}
		/>
	</Form>
{:else}
	<p>
		Build steps with <code>on = "runner"</code> in genco.toml run on a runner;
		without one online (or none takes it within 5 minutes) they run here.
	</p>
	<ListView
		items={runners}
		key={r => r.name}
		bind:selected
		columns={[
			{ label: 'Name' },
			{ label: 'State' },
			{ label: 'Machine' },
			{ label: 'Version' },
			{ label: 'Last seen' },
		]}
		empty="No runners yet."
		minHeight={110}
		class="runners"
	>
		{#snippet row(r)}
			<td>{r.name}</td>
			<td class:off={!r.online}>{runnerState(r)}</td>
			<td>
				{r.info.host ?? ''}{r.info.cpus ? `, ${r.info.cpus} CPUs` : ''}
			</td>
			<td class:off={outdated.includes(r)}>{r.info.version ?? ''}</td>
			<td>{r.lastSeen ? formatTime(r.lastSeen) : 'never'}</td>
		{/snippet}
	</ListView>
	{#if error}<p class="error">{error}</p>{/if}
	{#if outdated.length}
		<p class="error">
			{outdated.map(r => r.name).join(', ')} run another genco than this server
			({version}); on the build box: <code>genco runner update</code> (a
			daily timer does it after <code>genco runner install</code>).
		</p>
	{/if}
	{#if created}
		<OneTimeSecret
			label="On the build box, copy it now (the token is shown only once):"
			value={`genco runner install\n${created}\nsystemctl restart genco-runner`}
		/>
	{/if}
	{#if target}
		<p class="hint">
			Builds for {target.os}
			{target.version} ({target.codename}), glibc {target.glibc}, {target.arch}.
		</p>
	{/if}
	<div class="row-buttons">
		<Button onclick={() => (adding = true)}>New Runner…</Button>
		<Button onclick={remove} disabled={!selected}>Remove</Button>
	</div>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 0 0 8px;
	}

	:global(.list-view.runners) {
		margin-bottom: 8px;
	}

	.off {
		color: $hint;
	}
</style>
