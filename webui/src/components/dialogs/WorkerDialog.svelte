<script lang="ts">
	import { post, type Worker } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	// a new worker of a group, or one to change
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

	async function save() {
		let p: number | null = null;
		if (port.trim()) {
			p = Number(port);
			if (!Number.isInteger(p)) throw new Error('The port is a number');
		}
		const body = { name, dir, command, port: p };
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
		<input id="w-name" autofocus placeholder="queue" bind:value={name} />
		<label for="w-dir">Directory:</label>
		<input
			id="w-dir"
			placeholder="current/craft"
			title="relative to the group's directory"
			bind:value={dir}
		/>
		<label for="w-cmd">Command:</label>
		<input
			id="w-cmd"
			class="mono"
			spellcheck="false"
			placeholder="php craft queue/listen --verbose"
			bind:value={command}
		/>
		<label for="w-port">Public port:</label>
		<input
			id="w-port"
			inputmode="numeric"
			placeholder="none"
			title="open to the internet, the worker gets it as PORT"
			bind:value={port}
		/>
	</Form>
	<p class="hint">
		Runs as the group's user, restarted when it stops and after every
		deploy. Server → Apply Changes starts it.
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
