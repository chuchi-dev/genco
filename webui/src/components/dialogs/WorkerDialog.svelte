<script lang="ts">
	import { post, type Worker } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	// a new worker of a group, or one to change; of genco.toml's only the
	// address
	let {
		groupId,
		worker = null,
		onclose,
	}: {
		groupId: number;
		worker?: Worker | null;
		onclose: () => void;
	} = $props();

	// svelte-ignore state_referenced_locally
	let name = $state(worker?.name ?? '');
	// svelte-ignore state_referenced_locally
	let dir = $state(worker?.dir ?? 'current');
	// svelte-ignore state_referenced_locally
	let command = $state(worker?.command ?? '');
	// svelte-ignore state_referenced_locally
	let port = $state(worker?.port ? String(worker.port) : '');
	// svelte-ignore state_referenced_locally
	let address = $state(worker?.address ?? '');
	const repo = $derived(worker?.source === 'repo');

	async function save() {
		if (worker && repo) {
			await post(`/workers/${worker.id}/address`, { address });
			onclose();
			return;
		}
		let p: number | null = null;
		if (port.trim()) {
			p = Number(port);
			if (!Number.isInteger(p)) throw new Error('The port is a number');
		}
		const body = { name, dir, command, port: p, address };
		if (worker) await post(`/workers/${worker.id}`, body);
		else await post(`/groups/${groupId}/workers`, body);
		onclose();
	}
</script>

<Dialog
	title={worker ? `Worker ${worker.name}` : 'New Worker'}
	icon="gear"
	{onclose}
	width={460}
>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="w-name">Name:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			id="w-name"
			autofocus={!repo}
			disabled={repo}
			placeholder="queue"
			bind:value={name}
		/>
		<label for="w-dir">Directory:</label>
		<input
			id="w-dir"
			placeholder="current/craft"
			title="relative to the group's directory"
			disabled={repo}
			bind:value={dir}
		/>
		<label for="w-cmd">Command:</label>
		<input
			id="w-cmd"
			class="mono"
			spellcheck="false"
			placeholder="php craft queue/listen --verbose"
			disabled={repo}
			bind:value={command}
		/>
		<label for="w-port">Public port:</label>
		<input
			id="w-port"
			inputmode="numeric"
			placeholder="none"
			title="open to the internet, the worker gets it as PORT"
			disabled={repo}
			bind:value={port}
		/>
		<label for="w-address">Address:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			id="w-address"
			autofocus={repo}
			maxlength="200"
			placeholder="packages.example.com:5426"
			title="where clients reach it, a note for the panel"
			bind:value={address}
		/>
	</Form>
	<p class="hint">
		{#if repo}
			It comes with the deploys (genco.toml or detected), change it in the
			repository; the address is noted here only.
		{/if}
		Runs as the group's user, restarted when it stops and after every deploy.
		Server → Apply Changes starts it.
	</p>
</Dialog>

<style lang="scss">
	@use 'styles/win' as *;

	.mono {
		font-family: $mono;
	}

	.hint {
		margin: 8px 0 0;
	}
</style>
